//! Recovery: writes only to the destination folder. File names are untrusted input
//! (a malicious USB can carry a name like "../../.ssh/x"); all of them are sanitized.

use crate::ctx::*;
use crate::model::*;
use crate::reader;
use crate::source::Source;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::ffi::CString;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Instant;

#[derive(serde::Deserialize, Clone)]
#[serde(default)]
pub struct Opts {
    pub dest: String,
    pub keep_tree: bool,
    pub by_kind: bool,
    pub verify: bool,
    pub skip_lost: bool,
    pub restore_time: bool,
    pub report: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { dest: String::new(), keep_tree: true, by_kind: false, verify: true, skip_lost: false, restore_time: true, report: true }
    }
}

pub fn sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c == '/' || c == '\\' || c == '\0' || (c as u32) < 32 { '_' } else { c })
        .collect();
    s = s.trim_matches(|c: char| c == ' ').to_string();
    if s.is_empty() || s == "." || s == ".." {
        return "unnamed".to_string();
    }
    // If the file name byte limit (255) is exceeded, trim the end and keep the extension.
    if s.len() > 240 {
        let ext = ext_of(&s);
        let mut cut = 220;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        if !ext.is_empty() {
            s.push('.');
            s.push_str(&ext);
        }
    }
    s
}

fn kind_dir(k: Kind) -> &'static str {
    match k {
        Kind::Image => "Images",
        Kind::Video => "Videos",
        Kind::Audio => "Audio",
        Kind::Document => "Documents",
        Kind::Archive => "Archives",
        Kind::Code => "Text and code",
        Kind::App => "Applications",
        Kind::Database => "Databases",
        Kind::Other => "Other",
    }
}

fn exclusive(path: &Path) -> Option<(File, PathBuf)> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned());
    let dir = path.parent()?;
    for n in 0..100000 {
        let candidate = if n == 0 {
            path.to_path_buf()
        } else {
            match &ext {
                Some(x) => dir.join(format!("{stem} ({n}).{x}")),
                None => dir.join(format!("{stem} ({n})")),
            }
        };
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(libc::O_NOFOLLOW)
            .mode(0o644)
            .open(&candidate)
        {
            Ok(f) => return Some((f, candidate)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        }
    }
    None
}

fn set_times(f: &File, mtime: i64) {
    let ts = [
        libc::timespec { tv_sec: mtime, tv_nsec: 0 },
        libc::timespec { tv_sec: mtime, tv_nsec: 0 },
    ];
    unsafe {
        libc::futimens(f.as_raw_fd(), ts.as_ptr());
    }
}

pub fn free_space(path: &str) -> Option<u64> {
    let c = CString::new(Path::new(path).as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    (r == 0).then(|| st.f_bavail as u64 * st.f_frsize as u64)
}

fn now_label() -> String {
    let mut t: libc::tm = unsafe { std::mem::zeroed() };
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    unsafe { libc::localtime_r(&now, &mut t) };
    format!(
        "{:04}-{:02}-{:02} {:02}.{:02}.{:02}",
        t.tm_year + 1900,
        t.tm_mon + 1,
        t.tm_mday,
        t.tm_hour,
        t.tm_min,
        t.tm_sec
    )
}

pub fn run(src: Arc<Source>, cat: Arc<RwLock<Catalog>>, ids: Vec<u32>, o: Opts, ctl: Arc<Control>, em: Emitter) {
    let t0 = Instant::now();
    let root = Path::new(&o.dest).join(format!("Salvage Recovered {}", now_label()));
    if let Err(e) = fs::create_dir_all(&root) {
        em.event("recover_done", json!({"error": format!("cannot create destination: {e}")}));
        return;
    }
    let total: u64 = {
        let c = cat.read().unwrap();
        ids.iter().filter_map(|i| c.entries.get(*i as usize)).map(|e| e.size).sum()
    };
    let (mut ok, mut failed, mut skipped, mut bytes, mut partial) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut rows: Vec<serde_json::Value> = Vec::with_capacity(ids.len());
    let mut last = Instant::now();
    let mut done = 0u64;

    for (idx, id) in ids.iter().enumerate() {
        if !ctl.check() {
            break;
        }
        let (e, rel) = {
            let c = cat.read().unwrap();
            let Some(e) = c.entries.get(*id as usize) else { continue };
            (e.clone(), c.path_of(e.dir))
        };
        if o.skip_lost && e.chance <= 3 {
            skipped += 1;
            done += e.size;
            continue;
        }
        let mut dir = root.clone();
        if o.by_kind {
            dir.push(kind_dir(e.kind));
        } else if o.keep_tree {
            for part in rel.split('/') {
                if !part.is_empty() {
                    dir.push(sanitize(part));
                }
            }
        }
        if !dir.starts_with(&root) || fs::create_dir_all(&dir).is_err() {
            failed += 1;
            rows.push(json!({"id": id, "name": e.name, "status": "error", "error": "cannot create folder"}));
            continue;
        }
        let Some((mut f, path)) = exclusive(&dir.join(sanitize(&e.name))) else {
            failed += 1;
            rows.push(json!({"id": id, "name": e.name, "status": "error", "error": "cannot create file"}));
            continue;
        };
        let mut hasher = Sha256::new();
        let mut err = None;
        let mut local = 0u64;
        let written = reader::stream(
            &src,
            &e,
            |b| {
                if o.verify {
                    hasher.update(b);
                }
                if let Err(x) = f.write_all(b) {
                    err = Some(x);
                    return false;
                }
                local += b.len() as u64;
                if last.elapsed().as_millis() >= 200 {
                    last = Instant::now();
                    let secs = t0.elapsed().as_secs_f64().max(0.001);
                    let cur = done + local;
                    let rate = cur as f64 / secs;
                    em.event(
                        "recover",
                        json!({
                            "done_files": idx, "total_files": ids.len(),
                            "done_bytes": cur, "total_bytes": total,
                            "rate": rate, "eta": if rate > 1.0 { (total.saturating_sub(cur)) as f64 / rate } else { -1.0 },
                            "current": e.name,
                        }),
                    );
                }
                ctl.check()
            },
            u64::MAX,
        );
        if ctl.is_cancelled() {
            drop(f);
            let _ = fs::remove_file(&path);
            break;
        }
        if let Some(x) = err {
            drop(f);
            let _ = fs::remove_file(&path);
            let fatal = x.raw_os_error() == Some(libc::ENOSPC);
            failed += 1;
            rows.push(json!({"id": id, "name": e.name, "status": "error", "error": x.to_string()}));
            if fatal {
                em.log("error", "Destination is full, recovery stopped");
                break;
            }
            continue;
        }
        let _ = f.flush();
        if o.restore_time && e.mtime > 0 {
            set_times(&f, e.mtime);
        }
        drop(f);
        done += e.size;
        bytes += written;
        let digest = if o.verify {
            hasher.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()
        } else {
            String::new()
        };
        // Read back afterwards to check that the write was sound too.
        let mut status = "ok";
        if written < e.size {
            status = "partial";
            partial += 1;
        } else if o.verify {
            match sha_file(&path) {
                Some(h) if h == digest => {}
                _ => status = "unverified",
            }
        }
        if status == "ok" {
            ok += 1;
        }
        rows.push(json!({
            "id": id, "name": e.name, "path": path, "size": e.size, "written": written,
            "chance": e.chance, "method": e.method, "sha256": digest, "status": status,
        }));
    }

    if o.report {
        write_report(&root, &rows);
    }
    let cancelled = ctl.is_cancelled();
    em.event(
        "recover_done",
        json!({
            "dest": root, "ok": ok, "failed": failed, "skipped": skipped, "partial": partial,
            "bytes": bytes, "cancelled": cancelled, "seconds": t0.elapsed().as_secs_f64(),
        }),
    );
}

fn sha_file(p: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = File::open(p).ok()?;
    let mut h = Sha256::new();
    let mut b = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut b).ok()?;
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
    }
    Some(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn write_report(root: &Path, rows: &[serde_json::Value]) {
    let Ok(mut f) = File::create(root.join("recovery-report.csv")) else { return };
    let _ = writeln!(f, "status,size,chance,method,sha256,name,path");
    for r in rows {
        let q = |k: &str| {
            let v = r.get(k).map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).unwrap_or_default();
            format!("\"{}\"", v.replace('"', "\"\""))
        };
        let _ = writeln!(
            f,
            "{},{},{},{},{},{},{}",
            q("status"),
            q("size"),
            q("chance"),
            q("method"),
            q("sha256"),
            q("name"),
            q("path")
        );
    }
}
