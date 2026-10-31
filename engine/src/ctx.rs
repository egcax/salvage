//! Scan context: progress reporting, pause/cancel, disk map.

use crate::model::*;
use crate::source::{Source, Vol};
use serde_json::{json, Value};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::Instant;

pub const MAP_CELLS: usize = 1024;

pub const CELL_DATA: u8 = 1;
pub const CELL_ZERO: u8 = 2;
pub const CELL_FOUND: u8 = 3;
pub const CELL_META: u8 = 4;
pub const CELL_BAD: u8 = 5;

/// Shared emitter that writes JSON lines to stdout.
#[derive(Clone)]
pub struct Emitter {
    out: Arc<Mutex<std::io::Stdout>>,
}

impl Emitter {
    pub fn new() -> Emitter {
        Emitter { out: Arc::new(Mutex::new(std::io::stdout())) }
    }
    pub fn send(&self, v: &Value) {
        if let Ok(mut o) = self.out.lock() {
            let _ = serde_json::to_writer(&mut *o, v);
            let _ = o.write_all(b"\n");
            let _ = o.flush();
        }
    }
    pub fn event(&self, ev: &str, mut body: Value) {
        body["ev"] = json!(ev);
        self.send(&body);
    }
    pub fn log(&self, level: &str, msg: impl AsRef<str>) {
        self.event("log", json!({"level": level, "msg": msg.as_ref()}));
    }
}

#[derive(Default)]
pub struct Control {
    pub cancelled: AtomicBool,
    paused: Mutex<bool>,
    wake: Condvar,
}

impl Control {
    pub fn pause(&self, on: bool) {
        *self.paused.lock().unwrap() = on;
        self.wake.notify_all();
    }
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.pause(false);
    }
    pub fn reset(&self) {
        self.cancelled.store(false, Ordering::SeqCst);
        self.pause(false);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
    /// Waits while paused; returns false if cancelled.
    pub fn check(&self) -> bool {
        let mut p = self.paused.lock().unwrap();
        while *p && !self.is_cancelled() {
            p = self.wake.wait(p).unwrap();
        }
        !self.is_cancelled()
    }
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct ScanOpts {
    /// quick: metadata only; deep: metadata + free space signature scan; full: whole disk by signature.
    pub mode: String,
    pub kinds: Vec<Kind>,
    pub free_only: bool,
    pub existing: bool,
    /// Byte step: 512 sector, 4096 fast, 1 every byte.
    pub step: u32,
    pub min_size: u64,
    pub find_lost: bool,
    pub start: Option<u64>,
    pub end: Option<u64>,
    /// Copies deleted files here as soon as metadata is done.
    pub rescue_dest: Option<String>,
    pub rescue_limit: u64,
    /// Bytes per second limit, 0 for none.
    pub max_rate: u64,
    /// Lowers disk priority.
    pub background: bool,
    /// Resumes an interrupted scan where it stopped.
    pub resume: bool,
    /// Tries to join fragmented JPEGs.
    pub repair: bool,
}

impl Default for ScanOpts {
    fn default() -> Self {
        ScanOpts {
            mode: "deep".into(),
            kinds: vec![],
            free_only: true,
            existing: false,
            step: 512,
            min_size: 0,
            find_lost: true,
            start: None,
            end: None,
            rescue_dest: None,
            rescue_limit: 0,
            max_rate: 0,
            background: false,
            resume: false,
            repair: true,
        }
    }
}

pub struct Ctx {
    pub src: Arc<Source>,
    pub cat: Arc<RwLock<Catalog>>,
    pub ctl: Arc<Control>,
    pub em: Emitter,
    pub opts: ScanOpts,
    pub phase: Mutex<String>,
    pub map: Mutex<Vec<u8>>,
    pub map_dirty: AtomicBool,
    last: Mutex<Instant>,
    started: Instant,
    pub phase_done: AtomicU64,
    pub phase_total: AtomicU64,
    phase_start: Mutex<Instant>,
    pub found: AtomicU64,
    pub found_bytes: AtomicU64,
}

impl Ctx {
    pub fn new(
        src: Arc<Source>,
        cat: Arc<RwLock<Catalog>>,
        ctl: Arc<Control>,
        em: Emitter,
        opts: ScanOpts,
    ) -> Ctx {
        Ctx {
            src,
            cat,
            ctl,
            em,
            opts,
            phase: Mutex::new(String::new()),
            map: Mutex::new(vec![0u8; MAP_CELLS]),
            map_dirty: AtomicBool::new(false),
            last: Mutex::new(Instant::now()),
            started: Instant::now(),
            phase_done: AtomicU64::new(0),
            phase_total: AtomicU64::new(0),
            phase_start: Mutex::new(Instant::now()),
            found: AtomicU64::new(0),
            found_bytes: AtomicU64::new(0),
        }
    }

    pub fn alive(&self) -> bool {
        self.ctl.check()
    }

    pub fn set_phase(&self, phase: &str, total: u64) {
        *self.phase.lock().unwrap() = phase.to_string();
        self.phase_done.store(0, Ordering::Relaxed);
        self.phase_total.store(total, Ordering::Relaxed);
        *self.phase_start.lock().unwrap() = Instant::now();
        self.emit_progress(true);
    }

    pub fn advance(&self, done: u64) {
        self.phase_done.store(done, Ordering::Relaxed);
        self.emit_progress(false);
    }

    /// Progress event limited to 5 Hz; enough for the battery and the interface.
    pub fn emit_progress(&self, force: bool) {
        {
            let mut l = self.last.lock().unwrap();
            if !force && l.elapsed().as_millis() < 200 {
                return;
            }
            *l = Instant::now();
        }
        let done = self.phase_done.load(Ordering::Relaxed);
        let total = self.phase_total.load(Ordering::Relaxed);
        let secs = self.phase_start.lock().unwrap().elapsed().as_secs_f64().max(0.001);
        let rate = done as f64 / secs;
        let eta = if rate > 1.0 && total > done { (total - done) as f64 / rate } else { -1.0 };
        let count = self.found.load(Ordering::Relaxed);
        let bytes = self.found_bytes.load(Ordering::Relaxed);
        let mut body = json!({
            "phase": *self.phase.lock().unwrap(),
            "done": done,
            "total": total,
            "rate": rate,
            "eta": eta,
            "elapsed": self.started.elapsed().as_secs_f64(),
            "found": count,
            "found_bytes": bytes,
            "errors": self.src.read_errors.load(Ordering::Relaxed),
        });
        if self.map_dirty.swap(false, Ordering::Relaxed) {
            let m = self.map.lock().unwrap();
            let s: String = m.iter().map(|c| (b'0' + *c) as char).collect();
            body["map"] = json!(s);
        }
        self.em.event("progress", body);
    }

    pub fn mark(&self, start: u64, end: u64, val: u8) {
        let size = self.src.size.max(1);
        let a = ((start as u128 * MAP_CELLS as u128) / size as u128) as usize;
        let b = ((end.max(start + 1) as u128 * MAP_CELLS as u128).div_ceil(size as u128)) as usize;
        let mut m = self.map.lock().unwrap();
        for i in a..b.min(MAP_CELLS) {
            // Findings and errors must not be overwritten by routine data that follows.
            if val >= m[i] || m[i] == 0 {
                m[i] = val;
            }
        }
        self.map_dirty.store(true, Ordering::Relaxed);
    }

    /// Existing file: listed only if requested or if it is in a trash folder.
    pub fn add_live(&self, mut e: Entry) {
        if self.cat.read().unwrap().in_trash(e.dir) {
            e.state = State::Trash;
        } else if !self.opts.existing {
            return;
        }
        self.add(e);
    }

    pub fn add(&self, e: Entry) {
        if e.state != State::Existing {
            self.found.fetch_add(1, Ordering::Relaxed);
            self.found_bytes.fetch_add(e.size, Ordering::Relaxed);
        }
        self.cat.write().unwrap().add(e);
    }

    pub fn log(&self, msg: impl AsRef<str>) {
        self.em.log("info", msg);
    }
    pub fn warn(&self, msg: impl AsRef<str>) {
        self.em.log("warn", msg);
    }
}

/// Common result of the file system parsers.
pub struct FsResult {
    pub fstype: String,
    pub label: String,
    pub block_size: u32,
    /// Used space in absolute offsets.
    pub alloc: Intervals,
    pub note: Option<String>,
}

impl Ctx {
    /// Speed limit: waits as long as needed based on total bytes read.
    pub fn throttle(&self, total_read: u64, started: Instant) {
        if self.opts.max_rate == 0 {
            return;
        }
        let want = total_read as f64 / self.opts.max_rate as f64;
        let had = started.elapsed().as_secs_f64();
        if want > had {
            std::thread::sleep(std::time::Duration::from_secs_f64((want - had).min(1.0)));
        }
    }
}

pub type FsScan = fn(&Vol, &Ctx, u16, u32) -> Result<FsResult, String>;
