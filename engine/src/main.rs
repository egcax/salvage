//! salvage-engine
//!
//!   salvage-engine devices               writes devices as JSON (no root needed)
//!   salvage-engine serve --source PATH   opens the source, reads commands from stdin
//!   salvage-engine serve --session FILE  loads a saved session
//!
//! When started as root (pkexec) it only opens the device and reads SMART, then
//! permanently drops to the calling user. All untrusted disk structures are
//! parsed in the unprivileged process.

mod carve;
mod check;
mod codec;
mod ctx;
mod devices;
mod fs;
mod image_copy;
mod model;
mod preview;
mod reader;
mod recover;
mod scan;
mod source;

use ctx::*;
use model::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use source::Source;
use std::io::{BufRead, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

fn smart_json(dev: &str) -> Option<Value> {
    let mut child = Command::new("/usr/bin/smartctl")
        .args(["-j", "-x", dev])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut out = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out.read_to_end(&mut b);
        let _ = tx.send(b);
    });
    let r = rx.recv_timeout(Duration::from_secs(8));
    let _ = child.kill();
    let _ = child.wait();
    serde_json::from_slice(&r.ok()?).ok()
}

/// Finds the mounted file systems of the source for TRIM protection.
fn discard_mounts(path: &str) -> Vec<(String, String)> {
    let name = path.rsplit('/').next().unwrap_or(path).to_string();
    let mut seen = std::collections::HashSet::new();
    std::fs::read_to_string("/proc/self/mounts")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split_whitespace().collect();
            if p.len() < 4 {
                return None;
            }
            let dev = p[0].rsplit('/').next().unwrap_or("");
            if !p[0].starts_with("/dev/") || !dev.starts_with(&name) || !seen.insert(p[0].to_string()) {
                return None;
            }
            let opt = p[3].split(',').find(|o| *o == "discard" || o.starts_with("discard="))?;
            Some((p[1].replace("\\040", " "), opt.to_string()))
        })
        .collect()
}

/// Stops TRIM during a scan. A small root process remains: when the engine exits
/// (even on a crash, as the pipe closes) it restores the original option.
fn hold_trim(path: &str) -> Vec<String> {
    let mounts = discard_mounts(path);
    let mut held = Vec::new();
    for (mp, opt) in &mounts {
        let ok = Command::new("/usr/bin/mount").env_clear().args(["-o", "remount,nodiscard", mp]).stderr(Stdio::null()).status();
        if ok.map(|s| s.success()).unwrap_or(false) {
            held.push((mp.clone(), opt.clone()));
        }
    }
    if held.is_empty() {
        return vec![];
    }
    unsafe {
        let mut fds = [0i32; 2];
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return held.into_iter().map(|h| h.0).collect();
        }
        let pid = libc::fork();
        if pid == 0 {
            libc::setsid();
            for fd in 0..1024 {
                if fd != fds[0] {
                    libc::close(fd);
                }
            }
            let null = libc::open(c"/dev/null".as_ptr(), libc::O_RDWR);
            libc::dup2(null, 0);
            libc::dup2(null, 1);
            libc::dup2(null, 2);
            let mut b = [0u8; 1];
            while libc::read(fds[0], b.as_mut_ptr().cast(), 1) > 0 {}
            for (mp, opt) in &held {
                let _ = Command::new("/usr/bin/mount").env_clear().args(["-o", &format!("remount,{opt}"), mp]).status();
            }
            libc::_exit(0);
        }
        libc::close(fds[0]);
        // The write end stays open for the life of the process.
        let _ = fds[1];
    }
    held.into_iter().map(|h| h.0).collect()
}

/// Permanent privilege drop. If it fails, the process does not continue.
fn drop_privileges() {
    unsafe {
        if libc::geteuid() != 0 {
            return;
        }
        let uid: libc::uid_t = std::env::var("PKEXEC_UID")
            .or_else(|_| std::env::var("SUDO_UID"))
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|u| *u != 0)
            .unwrap_or_else(|| {
                eprintln!("cannot determine calling user");
                std::process::exit(3);
            });
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            eprintln!("user not found");
            std::process::exit(3);
        }
        let gid = (*pw).pw_gid;
        let home = std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned();
        let name = std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned();
        let groups = [gid];
        let bad = libc::setgroups(1, groups.as_ptr()) != 0
            || libc::setresgid(gid, gid, gid) != 0
            || libc::setresuid(uid, uid, uid) != 0
            || libc::setuid(0) == 0;
        if bad || libc::geteuid() != uid {
            eprintln!("cannot drop privileges");
            std::process::exit(3);
        }
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
        std::env::set_var("HOME", home);
        std::env::set_var("USER", name);
        std::env::remove_var("XDG_CACHE_HOME");
    }
}

#[derive(Deserialize)]
struct SessionFile {
    source: String,
    source_size: u64,
    created: i64,
    opts: ScanOpts,
    catalog: Catalog,
}

#[derive(Serialize)]
struct SessionOut<'a> {
    version: u32,
    source: &'a str,
    source_size: u64,
    created: i64,
    opts: ScanOpts,
    catalog: &'a Catalog,
}

struct App {
    src: Arc<Source>,
    source_path: String,
    cat: Arc<RwLock<Catalog>>,
    ctl: Arc<Control>,
    /// Recovery, imaging, verification, search: these run independently of the scan.
    task: Arc<Control>,
    tbusy: Arc<AtomicBool>,
    trim_held: Vec<String>,
    em: Emitter,
    busy: Arc<AtomicBool>,
    smart: Option<Value>,
    session: String,
    opts: Mutex<ScanOpts>,
    preview_tx: Mutex<mpsc::Sender<(Value, Value)>>,
}

fn row(c: &Catalog, e: &Entry) -> Value {
    json!({
        "id": e.id, "name": e.name, "ext": e.ext, "size": e.size, "state": e.state,
        "kind": e.kind, "method": e.method, "mtime": e.mtime, "ctime": e.ctime, "dtime": e.dtime,
        "chance": e.chance, "note": e.note, "vol": e.vol, "dir": e.dir, "path": c.path_of(e.dir),
        "dup": e.dup, "verified": e.verified,
    })
}

impl App {
    fn reply(&self, id: &Value, data: Result<Value, String>) {
        match data {
            Ok(d) => self.em.send(&json!({"id": id, "ok": true, "data": d})),
            Err(e) => self.em.send(&json!({"id": id, "ok": false, "error": e})),
        }
    }

    fn ids_of(&self, sel: &Value) -> Result<Vec<u32>, String> {
        if let Some(a) = sel.get("ids") {
            return serde_json::from_value(a.clone()).map_err(|e| e.to_string());
        }
        let f: Filter = serde_json::from_value(sel.get("filter").cloned().unwrap_or(json!({})))
            .map_err(|e| e.to_string())?;
        let except: std::collections::HashSet<u32> = sel
            .get("except")
            .and_then(|e| serde_json::from_value(e.clone()).ok())
            .unwrap_or_default();
        let mut c = self.cat.write().unwrap();
        let ids = c.select(&f, &Sort { by: "offset".into(), desc: false });
        Ok(ids.into_iter().filter(|i| !except.contains(i)).collect())
    }

    fn handle(self: &Arc<Self>, id: Value, m: Value) {
        let cmd = m["cmd"].as_str().unwrap_or("").to_string();
        match cmd.as_str() {
            "info" => {
                let c = self.cat.read().unwrap();
                let parts = fs::partitions(&self.src);
                self.reply(
                    &id,
                    Ok(json!({
                        "source": self.source_path, "size": self.src.size, "sector": self.src.sector,
                        "is_block": self.src.is_block, "smart": self.smart, "volumes": c.volumes,
                        "entries": c.entries.len(), "partitions": parts,
                        "uid": unsafe { libc::geteuid() }, "trim_held": self.trim_held,
                        "pending": c.pending.iter().map(|r| r.1 - r.0).sum::<u64>(),
                    })),
                );
            }
            "scan" => {
                if self.busy.swap(true, Ordering::SeqCst) {
                    return self.reply(&id, Err("another task is running".into()));
                }
                let opts: ScanOpts = serde_json::from_value(m["opts"].clone()).unwrap_or_default();
                *self.opts.lock().unwrap() = opts.clone();
                self.ctl.reset();
                if !opts.resume {
                    *self.cat.write().unwrap() = Catalog::default();
                }
                let cx = Arc::new(Ctx::new(self.src.clone(), self.cat.clone(), self.ctl.clone(), self.em.clone(), opts));
                let busy = self.busy.clone();
                let em = self.em.clone();
                std::thread::spawn(move || {
                    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| scan::run(cx)));
                    if r.is_err() {
                        em.event("scan_done", json!({"error": "scan stopped unexpectedly"}));
                    }
                    busy.store(false, Ordering::SeqCst);
                });
                self.reply(&id, Ok(json!({"started": true})));
            }
            "pause" => {
                self.ctl.pause(true);
                self.reply(&id, Ok(json!({})));
            }
            "resume" => {
                self.ctl.pause(false);
                self.reply(&id, Ok(json!({})));
            }
            "cancel" => {
                self.ctl.cancel();
                self.reply(&id, Ok(json!({})));
            }
            "cancel_task" => {
                self.task.cancel();
                self.reply(&id, Ok(json!({})));
            }
            "query" => {
                let f: Filter = serde_json::from_value(m["filter"].clone()).unwrap_or_default();
                let s: Sort = serde_json::from_value(m["sort"].clone()).unwrap_or_default();
                let off = m["offset"].as_u64().unwrap_or(0) as usize;
                let lim = m["limit"].as_u64().unwrap_or(200).min(2000) as usize;
                let mut c = self.cat.write().unwrap();
                let ids = c.select(&f, &s);
                let fx = c.facet_counts(&f);
                let bytes: u64 = ids.iter().map(|i| c.entries[*i as usize].size).sum();
                let items: Vec<Value> =
                    ids.iter().skip(off).take(lim).map(|i| row(&c, &c.entries[*i as usize])).collect();
                self.reply(&id, Ok(json!({"total": ids.len(), "bytes": bytes, "items": items, "facets": fx})));
            }
            "tree" => {
                // Folder tree: only branches holding visible records.
                let f: Filter = serde_json::from_value(m["filter"].clone()).unwrap_or_default();
                let mut c = self.cat.write().unwrap();
                let ids = c.select(&f, &Sort::default());
                let n = c.dirs.len();
                let mut count = vec![0u64; n];
                let mut bytes = vec![0u64; n];
                for i in &ids {
                    let e = &c.entries[*i as usize];
                    let mut d = e.dir;
                    let mut g = 0;
                    while d != NO_PARENT && g < 512 {
                        count[d as usize] += 1;
                        bytes[d as usize] += e.size;
                        d = c.dirs[d as usize].parent;
                        g += 1;
                    }
                }
                // The tree loads lazily: only the children of the requested folder are returned.
                let want = m["parent"].as_u64().map(|p| p as u32).unwrap_or(NO_PARENT);
                let mut dirs: Vec<Value> = c
                    .dirs
                    .iter()
                    .enumerate()
                    .filter(|(i, d)| count[*i] > 0 && d.parent == want)
                    .map(|(i, d)| {
                        let has_kids = c.dirs.iter().any(|x| x.parent == i as u32);
                        json!({"id": i, "parent": if d.parent == NO_PARENT { json!(null) } else { json!(d.parent) },
                               "name": d.name, "count": count[i], "bytes": bytes[i],
                               "vol": d.vol, "deleted": d.deleted, "has_children": has_kids})
                    })
                    .collect();
                dirs.sort_by(|a, b| b["count"].as_u64().cmp(&a["count"].as_u64()));
                self.reply(&id, Ok(json!({"dirs": dirs})));
            }
            "entry" => {
                let c = self.cat.read().unwrap();
                let i = m["entry"].as_u64().unwrap_or(0) as usize;
                let r = c.entries.get(i).map(|e| {
                    let mut v = row(&c, e);
                    v["pieces"] = json!(e.pieces.len());
                    v["first_off"] = json!(e.first_off());
                    v
                });
                self.reply(&id, r.ok_or_else(|| "no such entry".to_string()));
            }
            "read" | "thumb" | "extract" => {
                let _ = self.preview_tx.lock().unwrap().send((id, m));
            }
            "recover" => {
                if self.tbusy.swap(true, Ordering::SeqCst) {
                    return self.reply(&id, Err("another task is running".into()));
                }
                let o: recover::Opts = serde_json::from_value(m["opts"].clone()).unwrap_or_default();
                let ids = match self.ids_of(&m["sel"]) {
                    Ok(i) => i,
                    Err(e) => {
                        self.tbusy.store(false, Ordering::SeqCst);
                        return self.reply(&id, Err(e));
                    }
                };
                self.task.reset();
                let (src, cat, ctl, em, busy) =
                    (self.src.clone(), self.cat.clone(), self.task.clone(), self.em.clone(), self.tbusy.clone());
                let n = ids.len();
                std::thread::spawn(move || {
                    recover::run(src, cat, ids, o, ctl, em);
                    busy.store(false, Ordering::SeqCst);
                });
                self.reply(&id, Ok(json!({"started": true, "files": n})));
            }
            "image" => {
                if self.tbusy.swap(true, Ordering::SeqCst) {
                    return self.reply(&id, Err("another task is running".into()));
                }
                self.task.reset();
                let dest = m["dest"].as_str().unwrap_or("").to_string();
                let (src, ctl, em, busy) = (self.src.clone(), self.task.clone(), self.em.clone(), self.tbusy.clone());
                std::thread::spawn(move || {
                    image_copy::run(src, dest, ctl, em);
                    busy.store(false, Ordering::SeqCst);
                });
                self.reply(&id, Ok(json!({"started": true})));
            }
            "validate" | "grep" => {
                if self.tbusy.swap(true, Ordering::SeqCst) {
                    return self.reply(&id, Err("another task is running".into()));
                }
                self.task.reset();
                let (src, cat, ctl, em, busy) =
                    (self.src.clone(), self.cat.clone(), self.task.clone(), self.em.clone(), self.tbusy.clone());
                if cmd == "validate" {
                    let ids = match self.ids_of(&m["sel"]) {
                        Ok(i) => i,
                        Err(e) => {
                            self.tbusy.store(false, Ordering::SeqCst);
                            return self.reply(&id, Err(e));
                        }
                    };
                    let session = self.session.clone();
                    std::thread::spawn(move || {
                        check::run_validate(src, cat, ids, ctl, em, session);
                        busy.store(false, Ordering::SeqCst);
                    });
                } else {
                    let text = m["text"].as_str().unwrap_or("").to_string();
                    std::thread::spawn(move || {
                        check::run_grep(src, cat, text, ctl, em);
                        busy.store(false, Ordering::SeqCst);
                    });
                }
                self.reply(&id, Ok(json!({"started": true})));
            }
            "export" => {
                let f: Filter = serde_json::from_value(m["filter"].clone()).unwrap_or_default();
                let path = m["path"].as_str().unwrap_or("");
                let fmt = m["format"].as_str().unwrap_or("csv");
                let r = check::export(&mut self.cat.write().unwrap(), &f, path, fmt);
                self.reply(&id, r.map(|n| json!({"rows": n})));
            }
            "check_dest" => {
                let p = m["path"].as_str().unwrap_or("");
                self.reply(&id, Ok(devices::check_dest(&self.source_path, p)));
            }
            "save_session" => {
                let path = m["path"].as_str().unwrap_or("").to_string();
                let r = self.save(&path);
                self.reply(&id, r.map(|_| json!({"path": path})));
            }
            "quit" => {
                self.ctl.cancel();
                self.reply(&id, Ok(json!({})));
                std::process::exit(0);
            }
            other => self.reply(&id, Err(format!("unknown command: {other}"))),
        }
    }

    fn save(&self, path: &str) -> Result<(), String> {
        let tmp = format!("{path}.tmp");
        let cat = self.cat.read().unwrap();
        let file = SessionOut {
            version: 1,
            source: &self.source_path,
            source_size: self.src.size,
            created: unsafe { libc::time(std::ptr::null_mut()) },
            opts: self.opts.lock().unwrap().clone(),
            catalog: &cat,
        };
        let f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut z = zstd::stream::write::Encoder::new(f, 3).map_err(|e| e.to_string())?;
        serde_json::to_writer(&mut z, &file).map_err(|e| e.to_string())?;
        z.finish().map_err(|e| e.to_string())?.flush().map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }

    fn preview_worker(self: Arc<Self>, rx: Arc<Mutex<mpsc::Receiver<(Value, Value)>>>) {
        loop {
            let msg = rx.lock().unwrap().recv();
            let Ok((id, m)) = msg else { return };
            let i = m["entry"].as_u64().unwrap_or(0) as usize;
            let e = self.cat.read().unwrap().entries.get(i).cloned();
            let Some(e) = e else {
                self.reply(&id, Err("no such entry".into()));
                continue;
            };
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match m["cmd"].as_str() {
                Some("read") => {
                    let off = m["off"].as_u64().unwrap_or(0);
                    let n = m["len"].as_u64().unwrap_or(4096).min(1 << 20) as usize;
                    let b = reader::range(&self.src, &e, off, n);
                    let hex: String = b.iter().map(|c| format!("{c:02x}")).collect();
                    json!({"hex": hex, "off": off})
                }
                Some("thumb") => {
                    let sz = m["size"].as_u64().unwrap_or(256) as u32;
                    json!({"path": preview::thumb(&self.src, &e, sz, &self.session)})
                }
                _ => json!({"path": preview::extract(&self.src, &e, &self.session)}),
            }));
            self.reply(&id, r.map_err(|_| "preview failed".to_string()));
        }
    }
}

fn load_session(path: &str) -> Result<SessionFile, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let z = zstd::stream::read::Decoder::new(f).map_err(|e| e.to_string())?;
    let mut s: SessionFile = serde_json::from_reader(z).map_err(|e| e.to_string())?;
    s.catalog.rebuild_index();
    Ok(s)
}

fn is_user_name(s: &str) -> bool {
    !s.is_empty() && s.len() < 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

const RULE_FILE: &str = "/etc/polkit-1/rules.d/49-salvage.rules";

/// Turns password prompts on or off for this user. Runs through pkexec.
fn set_access(allow: bool) -> i32 {
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("must run through pkexec");
        return 1;
    }
    if !allow {
        let _ = std::fs::remove_file(RULE_FILE);
        return 0;
    }
    let uid: u32 = match std::env::var("PKEXEC_UID").ok().and_then(|s| s.parse().ok()) {
        Some(u) => u,
        None => return 1,
    };
    let pw = unsafe { libc::getpwuid(uid) };
    if pw.is_null() {
        return 1;
    }
    let name = unsafe { std::ffi::CStr::from_ptr((*pw).pw_name) }.to_string_lossy().into_owned();
    if !is_user_name(&name) {
        return 1;
    }
    // Polkit rules are read as ASCII only.
    let rule = format!(
        "// Salvage: local active session of this user may open disks without a password\n\
         polkit.addRule(function(action, subject) {{\n\
         \x20   if (action.id == \"dev.salvage.Salvage.engine\" && subject.user == \"{name}\" && subject.local && subject.active) {{\n\
         \x20       return polkit.Result.YES;\n\
         \x20   }}\n\
         }});\n"
    );
    let tmp = format!("{RULE_FILE}.tmp");
    if std::fs::write(&tmp, rule).is_err() || std::fs::rename(&tmp, RULE_FILE).is_err() {
        return 1;
    }
    0
}

/// Reads SMART only, which is why the passwordless permission is safe.
fn smart_cmd(dev: &str) -> i32 {
    use std::os::unix::fs::FileTypeExt;
    let ok = dev.starts_with("/dev/")
        && !dev.contains("..")
        && std::fs::metadata(dev).map(|m| m.file_type().is_block_device()).unwrap_or(false);
    if !ok {
        println!("{}", json!({"error": "not a block device"}));
        return 1;
    }
    match smart_json(&devices::parent_of(dev)) {
        Some(v) => {
            println!("{v}");
            0
        }
        None => {
            println!("{}", json!({"error": "no SMART data"}));
            1
        }
    }
}

fn serve(source: Option<String>, session: Option<String>, hold: bool) -> i32 {
    let em = Emitter::new();
    let mut saved = None;
    let path = match (&source, &session) {
        (Some(s), _) => s.clone(),
        (None, Some(s)) => match load_session(s) {
            Ok(f) => {
                let p = f.source.clone();
                saved = Some(f);
                p
            }
            Err(e) => {
                em.event("fatal", json!({"error": format!("cannot load session: {e}")}));
                return 1;
            }
        },
        _ => {
            em.event("fatal", json!({"error": "no source given"}));
            return 2;
        }
    };
    let root = unsafe { libc::geteuid() } == 0;
    let src = match Source::open(&path) {
        Ok(s) => s,
        Err(e) => {
            em.event("fatal", json!({"error": e}));
            return 1;
        }
    };
    // No arbitrary file may be read with privileges: only block devices are opened.
    if root && !src.is_block {
        em.event("fatal", json!({"error": "only block devices can be opened with privileges"}));
        return 1;
    }
    let smart = if src.is_block && root { smart_json(&devices::parent_of(&path)) } else { None };
    let trim_held = if root && hold { hold_trim(&path) } else { vec![] };
    drop_privileges();
    preview::sweep_stale();

    let cat = match saved {
        Some(f) => {
            if f.source_size != src.size {
                em.log("warn", "device size differs from the saved session; previews may be wrong");
            }
            f.catalog
        }
        None => Catalog::default(),
    };
    let (tx, rx) = mpsc::channel();
    let session_id = format!("s{}", std::process::id());
    let app = Arc::new(App {
        src,
        source_path: path,
        cat: Arc::new(RwLock::new(cat)),
        ctl: Arc::new(Control::default()),
        task: Arc::new(Control::default()),
        tbusy: Arc::new(AtomicBool::new(false)),
        trim_held,
        em: em.clone(),
        busy: Arc::new(AtomicBool::new(false)),
        smart,
        session: session_id.clone(),
        opts: Mutex::new(ScanOpts::default()),
        preview_tx: Mutex::new(tx),
    });
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..2 {
        let (a, r) = (app.clone(), rx.clone());
        std::thread::spawn(move || a.preview_worker(r));
    }
    em.event("ready", json!({"source": app.source_path, "size": app.src.size, "uid": unsafe { libc::geteuid() }}));

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let Ok(m) = serde_json::from_str::<Value>(&line) else { continue };
        let id = m["id"].clone();
        app.handle(id, m);
    }
    // The interface closed: clear the preview cache.
    let _ = std::fs::remove_dir_all(preview::cache_dir(&session_id));
    0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // Panics from corrupt disk structures must not pollute the stdout protocol.
    std::panic::set_hook(Box::new(|_| {}));
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let val = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let code = match cmd {
        "devices" => {
            println!("{}", devices::list());
            0
        }
        "serve" => serve(val("--source"), val("--session"), args.iter().any(|a| a == "--hold-trim")),
        "smart" => smart_cmd(args.get(1).map(String::as_str).unwrap_or("")),
        "allow-user" => set_access(true),
        "deny-user" => set_access(false),
        "session-info" => {
            let p = val("--session").unwrap_or_default();
            match load_session(&p) {
                Ok(s) => {
                    let n = s.catalog.entries.iter().filter(|e| e.state != State::Existing).count();
                    let b: u64 = s.catalog.entries.iter().filter(|e| e.state != State::Existing).map(|e| e.size).sum();
                    println!("{}", json!({"source": s.source, "created": s.created, "entries": n, "bytes": b, "mode": s.opts.mode}));
                    0
                }
                Err(e) => {
                    println!("{}", json!({"error": e}));
                    1
                }
            }
        }
        _ => {
            eprintln!("usage: salvage-engine devices | serve --source PATH | serve --session FILE");
            2
        }
    };
    std::process::exit(code);
}
