//! Disk imaging, ddrescue style. Healthy areas are copied quickly first, bad
//! regions are skipped and retried sector by sector at the end. A map file lets
//! an interrupted job resume where it stopped.

use crate::ctx::*;
use crate::source::Source;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs::OpenOptions;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::sync::Arc;
use std::time::Instant;

const CHUNK: usize = 1 << 20;
const RETRIES: u32 = 2;

#[derive(Serialize, Deserialize, Default)]
struct Map {
    size: u64,
    pos: u64,
    bad: Vec<(u64, u64)>,
}

fn map_path(dest: &str) -> String {
    format!("{dest}.salvage-map")
}

fn save(dest: &str, m: &Map) {
    let tmp = format!("{}.tmp", map_path(dest));
    if let Ok(s) = serde_json::to_vec(m) {
        if std::fs::write(&tmp, s).is_ok() {
            let _ = std::fs::rename(&tmp, map_path(dest));
        }
    }
}

fn zero(b: &[u8]) -> bool {
    b.iter().all(|c| *c == 0)
}

pub fn run(src: Arc<Source>, dest: String, ctl: Arc<Control>, em: Emitter) {
    let t0 = Instant::now();
    let mut map: Map = std::fs::read(map_path(&dest)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let resuming = map.size == src.size && map.pos > 0;
    if !resuming {
        map = Map { size: src.size, pos: 0, bad: vec![] };
    }
    let opened = OpenOptions::new()
        .write(true)
        .create(resuming)
        .create_new(!resuming)
        .custom_flags(libc::O_NOFOLLOW)
        .mode(0o644)
        .open(&dest);
    let f = match opened {
        Ok(f) => f,
        Err(e) => {
            em.event("image_done", json!({"error": format!("cannot create image file: {e}")}));
            return;
        }
    };
    if f.set_len(src.size).is_err() {
        em.event("image_done", json!({"error": "not enough space for the image"}));
        return;
    }
    let mut buf = vec![0u8; CHUNK];
    let start_pos = map.pos;
    let mut last = Instant::now();
    let mut last_save = Instant::now();
    src.advise_sequential();

    // Pass 1: fast copy, unreadable chunks are skipped.
    while map.pos < src.size {
        if !ctl.check() {
            break;
        }
        let n = ((src.size - map.pos) as usize).min(CHUNK);
        if src.read_strict(map.pos, &mut buf[..n]) {
            if !zero(&buf[..n]) && f.write_all_at(&buf[..n], map.pos).is_err() {
                save(&dest, &map);
                em.event("image_done", json!({"error": "write error"}));
                return;
            }
        } else {
            match map.bad.last_mut() {
                Some(l) if l.1 == map.pos => l.1 += n as u64,
                _ => map.bad.push((map.pos, map.pos + n as u64)),
            }
        }
        src.drop_cache(map.pos, n as u64);
        map.pos += n as u64;
        if last.elapsed().as_millis() >= 200 {
            last = Instant::now();
            let rate = (map.pos - start_pos) as f64 / t0.elapsed().as_secs_f64().max(0.001);
            em.event("image", json!({"pass": 1, "done": map.pos, "total": src.size, "rate": rate,
                                     "eta": (src.size - map.pos) as f64 / rate.max(1.0), "bad": map.bad.len()}));
        }
        if last_save.elapsed().as_secs() >= 5 {
            last_save = Instant::now();
            save(&dest, &map);
        }
    }

    // Pass 2: bad regions, sector by sector.
    let ss = src.sector as usize;
    if !ctl.is_cancelled() {
        for attempt in 0..RETRIES {
            let bad = std::mem::take(&mut map.bad);
            let total: u64 = bad.iter().map(|(a, b)| b - a).sum();
            let mut done = 0u64;
            let mut sector = vec![0u8; ss];
            for (a, b) in bad {
                let mut o = a;
                while o < b {
                    if !ctl.check() {
                        map.bad.push((o, b));
                        break;
                    }
                    if src.read_sector(o, &mut sector) {
                        if !zero(&sector) {
                            let _ = f.write_all_at(&sector, o);
                        }
                    } else {
                        match map.bad.last_mut() {
                            Some(l) if l.1 == o => l.1 += ss as u64,
                            _ => map.bad.push((o, o + ss as u64)),
                        }
                    }
                    o += ss as u64;
                    done += ss as u64;
                    if last.elapsed().as_millis() >= 200 {
                        last = Instant::now();
                        em.event("image", json!({"pass": 2 + attempt, "done": done, "total": total, "rate": 0, "eta": -1, "bad": map.bad.len()}));
                    }
                }
            }
            if map.bad.is_empty() || ctl.is_cancelled() {
                break;
            }
        }
    }

    let cancelled = ctl.is_cancelled();
    let bad_bytes: u64 = map.bad.iter().map(|(a, b)| b - a).sum();
    if cancelled {
        save(&dest, &map);
    } else {
        let _ = std::fs::remove_file(map_path(&dest));
        if !map.bad.is_empty() {
            let text: String = map.bad.iter().map(|(a, b)| format!("{a} {b}\n")).collect();
            let _ = std::fs::write(format!("{dest}.bad-regions"), text);
        }
    }
    em.event(
        "image_done",
        json!({"path": dest, "bytes": map.pos, "cancelled": cancelled, "resumable": cancelled,
               "bad_ranges": map.bad.len(), "bad_bytes": bad_bytes, "seconds": t0.elapsed().as_secs_f64()}),
    );
}
