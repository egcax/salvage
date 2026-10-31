//! Jobs that run on results afterwards: open-and-check verification, content search,
//! report export.

use crate::ctx::*;
use crate::model::*;
use crate::preview;
use crate::reader;
use crate::source::*;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};
use std::time::Instant;

const MAX_DECODE: u64 = 256 << 20;
const MAX_MEDIA: u64 = 128 << 20;

#[derive(PartialEq, Clone, Copy)]
pub enum Verdict {
    Ok,
    Corrupt,
    Unknown,
}

fn zip_crc(b: &[u8]) -> Verdict {
    // Opens every entry from the central directory and compares the CRC.
    let Some(eocd) = b.windows(4).rposition(|w| w == b"PK\x05\x06") else { return Verdict::Corrupt };
    let n = u16le(b, eocd + 10) as usize;
    let mut p = u32le(b, eocd + 16) as usize;
    if n == 0xFFFF || p == 0xFFFF_FFFF {
        return Verdict::Unknown;
    }
    for _ in 0..n {
        if u32le(b, p) != 0x0201_4b50 {
            return Verdict::Corrupt;
        }
        let method = u16le(b, p + 10);
        let crc = u32le(b, p + 16);
        let csize = u32le(b, p + 20) as usize;
        let usize_ = u32le(b, p + 24) as usize;
        let (nl, xl, cl) = (u16le(b, p + 28) as usize, u16le(b, p + 30) as usize, u16le(b, p + 32) as usize);
        let lo = u32le(b, p + 42) as usize;
        if u32le(b, lo) != 0x0403_4b50 {
            return Verdict::Corrupt;
        }
        let ds = lo + 30 + u16le(b, lo + 26) as usize + u16le(b, lo + 28) as usize;
        let Some(data) = b.get(ds..ds + csize) else { return Verdict::Corrupt };
        let got = match method {
            0 => crc_of(data),
            8 => {
                let mut d = flate2::read::DeflateDecoder::new(data);
                let mut out = Vec::with_capacity(usize_.min(1 << 24));
                if d.read_to_end(&mut out).is_err() {
                    return Verdict::Corrupt;
                }
                crc_of(&out)
            }
            _ => return Verdict::Unknown,
        };
        if got != crc {
            return Verdict::Corrupt;
        }
        p += 46 + nl + xl + cl;
    }
    Verdict::Ok
}

fn crc_of(d: &[u8]) -> u32 {
    let mut h = flate2::Crc::new();
    h.update(d);
    h.sum()
}

fn run_ok(cmd: &str, args: &[&str]) -> Option<bool> {
    let o = Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped()).output().ok()?;
    Some(o.status.success())
}

/// Tries to actually open the file.
pub fn validate(src: &Source, e: &Entry, session: &str) -> Verdict {
    let raster = matches!(e.ext.as_str(), "jpg" | "jpeg" | "jfif" | "png" | "gif" | "bmp" | "tif" | "tiff" | "webp");
    let zipped = matches!(e.ext.as_str(), "zip" | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub" | "apk" | "jar");
    if e.size == 0 {
        return Verdict::Unknown;
    }
    if raster || zipped || matches!(e.ext.as_str(), "gz" | "tgz" | "pdf" | "sqlite" | "sqlite3") {
        if e.size > MAX_DECODE {
            return Verdict::Unknown;
        }
        let b = reader::head(src, e, e.size as usize);
        if raster {
            // A variant the decoder does not support does not count as corruption.
            return match image::load_from_memory(&b) {
                Ok(_) => Verdict::Ok,
                Err(image::ImageError::Unsupported(_)) => Verdict::Unknown,
                Err(_) if matches!(e.ext.as_str(), "tif" | "tiff") => Verdict::Unknown,
                Err(_) => Verdict::Corrupt,
            };
        }
        if zipped {
            return zip_crc(&b);
        }
        if e.ext == "pdf" {
            let tail = &b[b.len().saturating_sub(2048)..];
            if !tail.windows(5).any(|w| w == b"%%EOF") {
                return Verdict::Corrupt;
            }
            return Verdict::Ok;
        }
        if e.ext.starts_with("sqlite") {
            let ps = match u16be(&b, 16) {
                1 => 65536u64,
                p => p as u64,
            };
            let pages = u32be(&b, 28) as u64;
            return if ps * pages == b.len() as u64 && b.starts_with(b"SQLite format 3\0") { Verdict::Ok } else { Verdict::Corrupt };
        }
        let mut d = flate2::read::GzDecoder::new(&b[..]);
        let mut sink = [0u8; 65536];
        loop {
            match d.read(&mut sink) {
                Ok(0) => return Verdict::Ok,
                Ok(_) => {}
                Err(_) => return Verdict::Corrupt,
            }
        }
    }
    if matches!(e.kind, Kind::Video | Kind::Audio) && e.size <= MAX_MEDIA {
        let dir = preview::cache_dir(session);
        let safe: String = e.ext.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
        let tmp = dir.join(format!("v{}.{}", e.id, if safe.is_empty() { "bin" } else { &safe }));
        if reader::to_file(src, e, &tmp, MAX_MEDIA).is_err() {
            return Verdict::Unknown;
        }
        let r = tmp.to_str().and_then(|p| run_ok("ffmpeg", &["-v", "error", "-xerror", "-i", p, "-f", "null", "-"]));
        let _ = std::fs::remove_file(&tmp);
        return match r {
            Some(true) => Verdict::Ok,
            Some(false) => Verdict::Corrupt,
            None => Verdict::Unknown,
        };
    }
    Verdict::Unknown
}

pub fn run_validate(src: Arc<Source>, cat: Arc<RwLock<Catalog>>, ids: Vec<u32>, ctl: Arc<Control>, em: Emitter, session: String) {
    let (mut good, mut bad, mut unknown) = (0u32, 0u32, 0u32);
    let n = ids.len();
    let mut last = Instant::now();
    for (i, id) in ids.iter().enumerate() {
        if !ctl.check() {
            break;
        }
        let Some(e) = cat.read().unwrap().entries.get(*id as usize).cloned() else { continue };
        let v = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| validate(&src, &e, &session))).unwrap_or(Verdict::Corrupt);
        match v {
            Verdict::Ok => good += 1,
            Verdict::Corrupt => bad += 1,
            Verdict::Unknown => unknown += 1,
        }
        if v != Verdict::Unknown {
            let mut c = cat.write().unwrap();
            let t = &mut c.entries[*id as usize];
            if v == Verdict::Ok {
                t.chance = t.chance.max(98);
                t.note = Some("note.valid".into());
            } else {
                t.chance = t.chance.min(18);
                t.note = Some("note.corrupt".into());
            }
            t.verified = true;
            c.version += 1;
        }
        if last.elapsed().as_millis() >= 200 {
            last = Instant::now();
            em.event("task", json!({"task": "validate", "done": i, "total": n, "good": good, "bad": bad}));
        }
    }
    em.event("task_done", json!({"task": "validate", "good": good, "bad": bad, "unknown": unknown, "cancelled": ctl.is_cancelled()}));
}

fn find_ci(hay: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || hay.len() < needle.len() {
        return false;
    }
    let first = needle[0];
    let up = first.to_ascii_uppercase();
    hay.windows(needle.len()).any(|w| {
        (w[0] == first || w[0] == up) && w.iter().zip(needle).all(|(a, b)| a.to_ascii_lowercase() == *b)
    })
}

/// Search in file contents: works on text and documents.
pub fn run_grep(src: Arc<Source>, cat: Arc<RwLock<Catalog>>, text: String, ctl: Arc<Control>, em: Emitter) {
    let needle: Vec<u8> = text.to_lowercase().into_bytes();
    let ids: Vec<u32> = {
        let c = cat.read().unwrap();
        c.entries
            .iter()
            .filter(|e| e.size <= 32 << 20 && e.chance > 3 && matches!(e.kind, Kind::Document | Kind::Code | Kind::Other))
            .map(|e| e.id)
            .collect()
    };
    let n = ids.len();
    let mut hits: Vec<u32> = Vec::new();
    let mut last = Instant::now();
    for (i, id) in ids.iter().enumerate() {
        if !ctl.check() {
            break;
        }
        let Some(e) = cat.read().unwrap().entries.get(*id as usize).cloned() else { continue };
        let mut found = false;
        let mut tail: Vec<u8> = Vec::new();
        reader::stream(
            &src,
            &e,
            |b| {
                let mut joined = std::mem::take(&mut tail);
                joined.extend_from_slice(b);
                found = find_ci(&joined, &needle);
                let keep = needle.len().saturating_sub(1).min(joined.len());
                tail = joined[joined.len() - keep..].to_vec();
                !found
            },
            32 << 20,
        );
        if found {
            hits.push(*id);
        }
        if last.elapsed().as_millis() >= 200 {
            last = Instant::now();
            em.event("task", json!({"task": "grep", "done": i, "total": n, "hits": hits.len()}));
        }
    }
    let count = hits.len();
    {
        let mut c = cat.write().unwrap();
        c.grep = Some(hits.into_iter().collect());
        c.version += 1;
    }
    em.event("task_done", json!({"task": "grep", "hits": count, "cancelled": ctl.is_cancelled()}));
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

pub fn export(cat: &mut Catalog, f: &Filter, path: &str, format: &str) -> Result<usize, String> {
    let ids = cat.select(f, &Sort { by: "name".into(), desc: false });
    let mut o = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let e2s = |e: std::io::Error| e.to_string();
    match format {
        "json" => {
            let rows: Vec<Value> = ids
                .iter()
                .map(|i| {
                    let e = &cat.entries[*i as usize];
                    json!({"name": e.name, "path": cat.path_of(e.dir), "size": e.size, "state": e.state,
                           "kind": e.kind, "mtime": e.mtime, "chance": e.chance, "method": e.method, "note": e.note})
                })
                .collect();
            serde_json::to_writer_pretty(&mut o, &rows).map_err(|e| e.to_string())?;
        }
        "html" => {
            writeln!(o, "<!doctype html><meta charset=utf-8><title>Salvage</title><style>body{{font:14px system-ui;margin:24px}}table{{border-collapse:collapse;width:100%}}td,th{{border-bottom:1px solid #8884;padding:6px 8px;text-align:left}}</style><h1>Salvage</h1><p>{} files</p><table><tr><th>Name<th>Path<th>Size<th>Modified<th>Chance<th>Method", ids.len()).map_err(e2s)?;
            for i in &ids {
                let e = &cat.entries[*i as usize];
                writeln!(o, "<tr><td>{}<td>{}<td>{}<td>{}<td>{}%<td>{}", esc(&e.name), esc(&cat.path_of(e.dir)), e.size, e.mtime, e.chance, esc(&e.method)).map_err(e2s)?;
            }
            writeln!(o, "</table>").map_err(e2s)?;
        }
        _ => {
            writeln!(o, "name,path,size,state,mtime,chance,method").map_err(e2s)?;
            let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
            for i in &ids {
                let e = &cat.entries[*i as usize];
                let st = serde_json::to_value(e.state).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
                writeln!(o, "{},{},{},{},{},{},{}", q(&e.name), q(&cat.path_of(e.dir)), e.size, st, e.mtime, e.chance, q(&e.method)).map_err(e2s)?;
            }
        }
    }
    Ok(ids.len())
}
