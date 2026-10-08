//! bo2zm: open Black Ops II image packs read-only. `t6ipak <pack.ipak>...`
//! lists each pack's sections and entries; `--decode` also decodes every
//! entry, parses it as an IWI v27 image and checks the image's own size
//! table. Nothing is written.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

use ipak_t6::{IpakHeader, decode_entry, parse_header, parse_index, parse_iwi};

fn read_at(file: &mut File, at: u64, len: usize) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(at))?;
    let mut buf = vec![0u8; len];
    file.read_exact(&mut buf)?;
    Ok(buf)
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let decode = args.first().is_some_and(|a| a == "--decode");
    if decode {
        args.remove(0);
    }
    if args.is_empty() {
        eprintln!("usage: t6ipak [--decode] <pack.ipak>...");
        return ExitCode::from(2);
    }
    let mut failed = false;
    for path in &args {
        if let Err(e) = report(Path::new(path), decode) {
            println!("== {path}\n   FAILED: {e}");
            failed = true;
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn report(path: &Path, decode: bool) -> Result<(), String> {
    let started = Instant::now();
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let file_len = file.metadata().map_err(|e| e.to_string())?.len();
    let head = read_at(&mut file, 0, 256.min(file_len as usize)).map_err(|e| e.to_string())?;
    let header: IpakHeader = parse_header(&head).map_err(|e| e.to_string())?;
    let index_bytes = read_at(
        &mut file,
        header.index.offset as u64,
        header.index.item_count as usize * ipak_t6::INDEX_ENTRY_LEN,
    )
    .map_err(|e| e.to_string())?;
    let index =
        parse_index(&index_bytes, header.index.item_count as usize).map_err(|e| e.to_string())?;
    let stored: u64 = index.iter().map(|e| e.size as u64).sum();
    println!(
        "== {}  ({} bytes; header says {})\n   {} entries, {} stored bytes; data section at {:#x} size {:#x}",
        path.display(),
        file_len,
        header.file_size,
        index.len(),
        stored,
        header.data.offset,
        header.data.size
    );
    if !decode {
        return Ok(());
    }
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    let mut bad = 0usize;
    let mut out_bytes = 0u64;
    let mut first_error = None;
    let mut out = Vec::new();
    // File order: the index is sorted by key, which jumps around the disk.
    let mut by_offset: Vec<_> = index.iter().collect();
    by_offset.sort_by_key(|e| e.offset);
    for entry in by_offset {
        let at = header.data.offset as u64 + entry.offset as u64;
        let bytes = read_at(&mut file, at, entry.size as usize).map_err(|e| e.to_string())?;
        out.clear();
        let result = decode_entry(&bytes, at, &mut out)
            .map_err(|e| e.to_string())
            .and_then(|()| {
                parse_iwi(&out).map_err(|e| e.to_string()).map(|img| {
                    format!(
                        "{:?}{}{}",
                        img.format,
                        if img.is_cube() { " cube" } else { "" },
                        if img.is_volume() { " volume" } else { "" }
                    )
                })
            });
        // T6IPAK_FIND=<name hash>: that image's format and size.
        if let Ok(want) = std::env::var("T6IPAK_FIND")
            && u32::from_str_radix(want.trim_start_matches("0x"), 16).ok()
                == Some(entry.name_hash())
        {
            println!(
                "   found {:#010x}: {:?} ({} bytes)",
                entry.name_hash(),
                result,
                out.len()
            );
            if let Ok(dest) = std::env::var("T6IPAK_OUT") {
                let _ = std::fs::write(&dest, &out);
            }
            if let Ok(img) = parse_iwi(&out) {
                println!("      {}x{} format {:?}", img.width, img.height, img.format);
            }
        }
        match result {
            Ok(kind) => {
                *formats.entry(kind).or_default() += 1;
                out_bytes += out.len() as u64;
            }
            Err(e) => {
                bad += 1;
                first_error.get_or_insert((entry.name_hash(), entry.data_hash(), e));
            }
        }
    }
    println!(
        "   decoded {} of {} entries to {} bytes of IWI images in {:.2?}",
        index.len() - bad,
        index.len(),
        out_bytes,
        started.elapsed()
    );
    for (k, n) in &formats {
        println!("      {n:>6}  {k}");
    }
    if let Some((name, data, e)) = first_error {
        println!("   first failure: name hash {name:#010x} data hash {data:#010x}: {e}");
        return Err(format!("{bad} entries failed"));
    }
    Ok(())
}
