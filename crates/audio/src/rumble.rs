use std::sync::Arc;

use bevy::input::gamepad::{GamepadRumbleIntensity, GamepadRumbleRequest};
use bevy::prelude::*;
use net::{LocalPresentClient, PresentedSnapshot};

#[derive(Clone, Debug)]
pub(crate) struct Rumble {
    duration_ms: i32,
    low: Vec<(f32, f32)>,
    high: Vec<(f32, f32)>,
}

impl Rumble {
    pub(crate) fn prepare(
        bank: &asset_audio::SoundCatalog,
        namespace: asset_core::AssetNamespace,
        name: &str,
    ) -> Result<Arc<Self>, String> {
        let text = |name: &str| -> Result<&str, String> {
            let key = format!("rumble/{name}");
            let bytes = bank
                .rawfiles
                .get(&(namespace, key.clone()))
                .ok_or_else(|| format!("missing {key}"))?;
            std::str::from_utf8(bytes).map_err(|_| format!("invalid text in {key}"))
        };
        let definition = text(name)?.trim_end_matches('\0');
        let mut fields = definition.split('\\');
        if fields.next() != Some("RUMBLE") {
            return Err(format!("invalid rumble header: {name}"));
        }
        let mut duration = None;
        let mut low = None;
        let mut high = None;
        while let Some(key) = fields.next() {
            let value = fields
                .next()
                .ok_or_else(|| format!("missing {key} value: {name}"))?;
            match key {
                "duration" => duration = value.parse::<f32>().ok(),
                "lowRumbleFile" => low = Some(value),
                "highRumbleFile" => high = Some(value),
                _ => {}
            }
        }
        let duration = duration
            .filter(|d| d.is_finite() && *d > 0.0)
            .ok_or_else(|| format!("invalid rumble duration: {name}"))?;
        let graph = |name: Option<&str>| -> Result<Vec<(f32, f32)>, String> {
            let name = name
                .filter(|name| !name.is_empty())
                .ok_or_else(|| "missing rumble graph name".to_owned())?;
            let mut tokens = text(name)?.trim_end_matches('\0').split_whitespace();
            if tokens.next() != Some("RUMBLEGRAPHFILE") {
                return Err(format!("invalid rumble graph header: {name}"));
            }
            let count = tokens
                .next()
                .and_then(|n| n.parse::<usize>().ok())
                .filter(|&n| n > 0 && n <= 16)
                .ok_or_else(|| format!("invalid rumble graph count: {name}"))?;
            let mut points = Vec::with_capacity(count);
            for _ in 0..count {
                let mut number = || {
                    tokens
                        .next()
                        .and_then(|n| n.parse::<f32>().ok())
                        .filter(|v| v.is_finite())
                        .ok_or_else(|| format!("invalid rumble graph point: {name}"))
                };
                let x = number()?;
                let y = number()?;
                if points.last().is_some_and(|&(previous, _)| previous > x) {
                    return Err(format!("unordered rumble graph: {name}"));
                }
                points.push((x, y));
            }
            Ok(points)
        };
        Ok(Arc::new(Self {
            duration_ms: (duration * 1000.0).max(1.0) as i32,
            low: graph(low)?,
            high: graph(high)?,
        }))
    }

    fn intensity(&self, elapsed: i32) -> GamepadRumbleIntensity {
        let fraction = (elapsed as f32 / self.duration_ms as f32).clamp(0.0, 1.0);
        GamepadRumbleIntensity {
            strong_motor: sample(&self.low, fraction),
            weak_motor: sample(&self.high, fraction),
        }
    }
}

fn sample(points: &[(f32, f32)], fraction: f32) -> f32 {
    let mut previous = points[0];
    if fraction <= previous.0 {
        return previous.1.clamp(0.0, 1.0);
    }
    for &next in &points[1..] {
        if fraction <= next.0 {
            let t = if next.0 > previous.0 {
                (fraction - previous.0) / (next.0 - previous.0)
            } else {
                1.0
            };
            return (previous.1 + (next.1 - previous.1) * t).clamp(0.0, 1.0);
        }
        previous = next;
    }
    previous.1.clamp(0.0, 1.0)
}

#[derive(Message)]
pub(crate) struct PlayRumble {
    pub bank_revision: u64,
    pub rumble: Arc<Rumble>,
}

#[derive(Default)]
struct RumblePlayback {
    owner: Option<(
        frame::WorldGeneration,
        sim::ClientId,
        sim::LifeSequence,
        u64,
    )>,
    last_time: i32,
    active: Vec<(i32, Arc<Rumble>)>,
    output: Option<(Entity, GamepadRumbleIntensity)>,
    selected: Option<Entity>,
    next_refresh_ms: i32,
}

fn push_rumble(active: &mut Vec<(i32, Arc<Rumble>)>, now: i32, rumble: Arc<Rumble>) {
    if active.len() >= 32
        && let Some((index, _)) = active
            .iter()
            .enumerate()
            .min_by_key(|(_, (start, rumble))| start.saturating_add(rumble.duration_ms))
    {
        active.swap_remove(index);
    }
    active.push((now, rumble));
}

pub(crate) fn register(app: &mut App) {
    app.add_message::<PlayRumble>().add_systems(
        Update,
        update
            .in_set(net::ClientSet::Effects)
            .after(crate::entity_events::play_viewmodel_notetrack_messages),
    );
}

fn update(
    mut requests: MessageReader<PlayRumble>,
    bank: Option<Res<crate::SoundBank>>,
    presented: Res<PresentedSnapshot>,
    local: Res<LocalPresentClient>,
    generation: Res<frame::WorldGeneration>,
    time: Res<Time>,
    devices: Res<frame::InputDevices>,
    mut tests: MessageReader<frame::TestControllerRumble>,
    gamepads: Query<Entity, With<Gamepad>>,
    mut state: Local<RumblePlayback>,
    mut output: MessageWriter<GamepadRumbleRequest>,
    settings: Res<frame::GameSettings>,
    active: Option<Res<frame::ActivePad>>,
) {
    let now = (time.elapsed_secs_f64() * 1000.0) as i32;
    let selected = active
        .and_then(|active| active.0)
        .filter(|entity| gamepads.contains(*entity));
    if state.selected != selected {
        state.active.clear();
        if let Some((gamepad, _)) = state.output.take() {
            output.write(GamepadRumbleRequest::Stop { gamepad });
        }
        state.selected = selected;
        state.next_refresh_ms = 0;
    }
    if !settings.pad_vibration || !devices.focused || selected.is_none() {
        requests.clear();
        tests.clear();
        state.active.clear();
        if let Some((gamepad, _)) = state.output.take() {
            output.write(GamepadRumbleRequest::Stop { gamepad });
        }
        return;
    }
    let owner = bank.as_ref().and_then(|bank| {
        let meta = presented.snapshot()?.meta.for_client(local.0)?;
        (meta.lifecycle == sim::ClientLifecycle::Alive).then_some((
            *generation,
            local.0,
            meta.life_sequence,
            bank.0.revision(),
        ))
    });
    if owner != state.owner || now < state.last_time {
        state.active.clear();
        if let Some((gamepad, _)) = state.output.take() {
            output.write(GamepadRumbleRequest::Stop { gamepad });
        }
        state.owner = owner;
    }
    state.last_time = now;
    state
        .active
        .retain(|(start, rumble)| now.saturating_sub(*start) < rumble.duration_ms);
    for request in requests.read() {
        if owner.is_none_or(|owner| owner.3 != request.bank_revision) {
            continue;
        }
        push_rumble(&mut state.active, now, Arc::clone(&request.rumble));
    }
    if tests.read().next().is_some() {
        push_rumble(
            &mut state.active,
            now,
            Arc::new(Rumble {
                duration_ms: 400,
                low: vec![(0.0, 0.6), (1.0, 0.0)],
                high: vec![(0.0, 0.4), (1.0, 0.0)],
            }),
        );
        tests.clear();
    }
    let mut intensity = GamepadRumbleIntensity {
        strong_motor: 0.0,
        weak_motor: 0.0,
    };
    for (start, rumble) in &state.active {
        let current = rumble.intensity(now.saturating_sub(*start));
        intensity.strong_motor = intensity.strong_motor.max(current.strong_motor);
        intensity.weak_motor = intensity.weak_motor.max(current.weak_motor);
    }
    if let Some(gamepad) = selected {
        let zero = intensity.strong_motor == 0.0 && intensity.weak_motor == 0.0;
        if zero {
            if state.output.take().is_some() {
                output.write(GamepadRumbleRequest::Stop { gamepad });
            }
            state.next_refresh_ms = 0;
        } else if state.output.is_none() || now >= state.next_refresh_ms {
            output.write(GamepadRumbleRequest::Stop { gamepad });
            output.write(GamepadRumbleRequest::Add {
                gamepad,
                duration: std::time::Duration::from_millis(160),
                intensity,
            });
            state.output = Some((gamepad, intensity));
            state.next_refresh_ms = now.saturating_add(80);
        }
    }
}
