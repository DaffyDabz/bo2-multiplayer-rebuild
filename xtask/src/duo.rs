use std::{
    fs,
    io::{BufRead, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::shell::Res;

struct Player {
    child: Child,
    directory: PathBuf,
}

impl Drop for Player {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
        let _ = self.send("quit !");
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Player {
    fn send(&mut self, script: &str) -> Res<()> {
        writeln!(
            self.child.stdin.as_mut().ok_or("console pipe closed")?,
            "{script}"
        )
        .map_err(|e| e.to_string())
    }

    fn status(&mut self) -> Res<String> {
        if let Some(status) = self.child.try_wait().map_err(|e| e.to_string())? {
            return Err(format!("{} exited: {status}", self.directory.display()));
        }
        match fs::read_to_string(self.directory.join("master.status")) {
            Ok(status) if status.contains("state=failed") || status.contains("state=closed") => {
                Err(format!(
                    "master connection ended; see {}",
                    self.directory.display()
                ))
            }
            Ok(status) => Ok(status),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
            Err(e) => Err(e.to_string()),
        }
    }
}

fn launch(root: &Path, run: &Path, binary: &Path, role: &str) -> Res<Player> {
    let directory = run.join(role);
    fs::create_dir_all(directory.join("iw4l-artifacts")).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        fs::create_dir_all(root.join("iw4l-artifacts/cache")).map_err(|e| e.to_string())?;
        std::os::unix::fs::symlink(
            root.join("iw4l-artifacts/cache"),
            directory.join("iw4l-artifacts/cache"),
        )
        .map_err(|e| e.to_string())?;
    }
    fs::write(
        directory.join("settings.cfg"),
        format!("resolution=960x540\nfullscreen=false\nvsync=true\nplayer_name={role}\n"),
    )
    .map_err(|e| e.to_string())?;
    let mut command = Command::new(binary);
    for key in ["IW4L_GAMES", "IW4L_MASTER_CA_CERT"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, root.join(value));
        }
    }
    let child = command
        .arg("menu")
        .current_dir(&directory)
        .env("IW4L_SETTINGS_PATH", directory.join("settings.cfg"))
        .env("IW4L_MASTER_STATUS_FILE", directory.join("master.status"))
        .env("IW4L_CONSOLE_STDIN", "1")
        .env("IW4L_PRESENT_MODE", "AutoVsync")
        .env_remove("IW4L_CMDS")
        .env_remove("IW4L_MASTER_JOIN")
        .env_remove("IW4L_MASTER_HOST_NAME")
        .stdin(Stdio::piped())
        .stdout(fs::File::create(directory.join("stdout.log")).map_err(|e| e.to_string())?)
        .stderr(fs::File::create(directory.join("stderr.log")).map_err(|e| e.to_string())?)
        .spawn()
        .map_err(|e| e.to_string())?;
    let player = Player { child, directory };
    fs::write(player.directory.join("pid"), player.child.id().to_string())
        .map_err(|e| e.to_string())?;
    Ok(player)
}

fn field<'a>(status: &'a str, key: &str) -> Option<&'a str> {
    status
        .lines()
        .filter_map(|line| line.split_once('='))
        .find_map(|(k, v)| (k == key).then_some(v))
}

fn wait_for(player: &mut Player, predicate: impl Fn(&str) -> bool) -> Res<String> {
    let start = Instant::now();
    loop {
        let status = player.status()?;
        if predicate(&status) {
            return Ok(status);
        }
        if start.elapsed() > Duration::from_secs(90) {
            return Err(format!("lobby timeout; see {}", player.directory.display()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn run(root: &Path) -> Res<()> {
    for key in ["IW4L_GAMES", "IW4L_MASTER_ADDR", "IW4L_MASTER_SERVER_NAME"] {
        if std::env::var(key).unwrap_or_default().is_empty() {
            return Err(format!("set {key} in .env or the environment"));
        }
    }
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "play".into());
    let binary = root
        .join("target")
        .join(if profile == "dev" { "debug" } else { &profile })
        .join(format!("iw4l{}", std::env::consts::EXE_SUFFIX));
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    let run = root
        .join("iw4l-artifacts/duo")
        .join(format!("{stamp}-{}", std::process::id()));
    fs::create_dir_all(&run).map_err(|e| e.to_string())?;
    println!("Duo: {}", run.display());
    let zone = std::env::var("ZONE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "iw4:mp_rust".into());
    let zone = if zone.contains(':') {
        zone
    } else {
        format!("iw4:{zone}")
    };
    let mode = std::env::var("MODE").unwrap_or_else(|_| "dm".into());
    if [&zone, &mode]
        .iter()
        .any(|s| s.contains([';', '\n', '\r', '"']))
    {
        return Err("invalid ZONE or MODE".into());
    }
    let setup = format!("set ui_mapname {zone}; set ui_gametype {mode}");
    let mut host = launch(root, &run, &binary, "host")?;
    host.send(&format!("{setup}; ui_create_lobby; ui_lobby_privacy"))?;
    let status = wait_for(&mut host, |s| field(s, "state") == Some("hosting"))?;
    let room = field(&status, "room").ok_or("host status missing room ID")?;
    fs::write(run.join("lobby.id"), room).map_err(|e| e.to_string())?;
    println!("Lobby: {room}");
    let mut client = launch(root, &run, &binary, "client")?;
    client.send(&format!("{setup}; ui_join_lobby_id {room}"))?;
    wait_for(&mut client, |s| field(s, "state") == Some("joined"))?;
    wait_for(&mut host, |s| {
        field(s, "members")
            .and_then(|v| v.parse::<usize>().ok())
            .is_some_and(|n| n >= 2)
    })?;
    host.send("ui_start_match")?;
    for (player, key) in [(&mut host, "HOST_CMDS"), (&mut client, "CLIENT_CMDS")] {
        let commands = std::env::var(key).unwrap_or_else(|_| "spawn 0".into());
        player.send(&format!("wait world; {commands}"))?;
    }
    println!("Connected. Commands: host <script> | client <script> | both <script> | quit");
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        if host.child.try_wait().map_err(|e| e.to_string())?.is_some()
            || client
                .child
                .try_wait()
                .map_err(|e| e.to_string())?
                .is_some()
        {
            break;
        }
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(line) if line.trim() == "quit" => {
                let _ = host.send("quit !");
                let _ = client.send("quit !");
                break;
            }
            Ok(line) => {
                let Some((role, script)) = line.trim().split_once(' ') else {
                    eprintln!("use host <script>, client <script>, both <script>, or quit");
                    continue;
                };
                match role {
                    "host" => host.send(script)?,
                    "client" => client.send(script)?,
                    "both" => {
                        host.send(script)?;
                        client.send(script)?;
                    }
                    _ => eprintln!("unknown recipient: {role}"),
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(200))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
    Ok(())
}
