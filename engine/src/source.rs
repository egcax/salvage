//! Read-only source access. No code path writes to the source: the file is
//! opened with O_RDONLY, and only this handle is used after privileges are
//! dropped. Read errors do not stop a scan, bad sectors read as zero.

use std::fs::{File, OpenOptions};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const BLKGETSIZE64: libc::c_ulong = 0x8008_1272;
const BLKSSZGET: libc::c_ulong = 0x1268;
const BLKDISCARDZEROES: libc::c_ulong = 0x127c;

const ALIGN: usize = 4096;

pub struct Source {
    file: File,
    /// On a mounted device the page cache is not consistent with the file system:
    /// reads go straight to the disk with O_DIRECT to avoid stale pages.
    direct: Option<File>,
    pub path: String,
    pub size: u64,
    pub sector: u32,
    pub is_block: bool,
    pub read_errors: AtomicU64,
    pub bad_ranges: Mutex<Vec<(u64, u64)>>,
    /// Retry at sector precision: the cache reads 4 KiB pages.
    fine: Option<File>,
}

impl Source {
    pub fn open(path: &str) -> Result<Arc<Source>, String> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOCTTY)
            .open(path)
            .map_err(|e| format!("cannot open {path}: {e}"))?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        let is_block = (meta.mode() & libc::S_IFMT) == libc::S_IFBLK;
        let mut size = meta.len();
        let mut sector = 512u32;
        if is_block {
            let fd = file.as_raw_fd();
            unsafe {
                let mut sz: u64 = 0;
                if libc::ioctl(fd, BLKGETSIZE64 as _, &mut sz) == 0 {
                    size = sz;
                }
                let mut ss: libc::c_int = 0;
                if libc::ioctl(fd, BLKSSZGET as _, &mut ss) == 0 && ss > 0 {
                    sector = ss as u32;
                }
            }
        } else if !meta.is_file() {
            return Err(format!("{path} is not a supported source"));
        }
        let direct = if is_block && mounted(path) {
            OpenOptions::new().read(true).custom_flags(libc::O_CLOEXEC | libc::O_NOCTTY | libc::O_DIRECT).open(path).ok()
        } else {
            None
        };
        let fine = if is_block {
            OpenOptions::new().read(true).custom_flags(libc::O_CLOEXEC | libc::O_NOCTTY | libc::O_DIRECT).open(path).ok()
        } else {
            None
        };
        Ok(Arc::new(Source {
            file,
            direct,
            path: path.to_string(),
            size,
            sector,
            is_block,
            read_errors: AtomicU64::new(0),
            bad_ranges: Mutex::new(Vec::new()),
            fine,
        }))
    }

    /// Does a trimmed area read as zeros?
    #[allow(dead_code)]
    pub fn discard_zeroes(&self) -> bool {
        if !self.is_block {
            return false;
        }
        let mut v: libc::c_uint = 0;
        unsafe { libc::ioctl(self.file.as_raw_fd(), BLKDISCARDZEROES as _, &mut v) == 0 && v != 0 }
    }

    /// Drops the range just read to avoid memory and cache pressure.
    pub fn drop_cache(&self, off: u64, len: u64) {
        unsafe {
            libc::posix_fadvise(
                self.file.as_raw_fd(),
                off as libc::off_t,
                len as libc::off_t,
                libc::POSIX_FADV_DONTNEED,
            );
        }
    }

    pub fn advise_sequential(&self) {
        unsafe {
            libc::posix_fadvise(self.file.as_raw_fd(), 0, 0, libc::POSIX_FADV_SEQUENTIAL);
        }
    }

    /// Fault-tolerant read: fills the whole buffer, unreadable parts stay zero.
    pub fn read_at(&self, off: u64, buf: &mut [u8]) {
        if off >= self.size {
            buf.fill(0);
            return;
        }
        let avail = (self.size - off).min(buf.len() as u64) as usize;
        buf[avail..].fill(0);
        if let Some(d) = &self.direct {
            if self.read_direct(d, off, &mut buf[..avail]) {
                return;
            }
        }
        match self.file.read_exact_at(&mut buf[..avail], off) {
            Ok(()) => {}
            Err(_) => self.read_slow(off, &mut buf[..avail]),
        }
    }

    /// Reads without hiding errors; imaging detects bad regions this way.
    pub fn read_strict(&self, off: u64, buf: &mut [u8]) -> bool {
        match &self.direct {
            Some(d) => self.read_direct(d, off, buf),
            None => self.file.read_exact_at(buf, off).is_ok(),
        }
    }

    /// Reads one sector, with the cache and read-ahead buffering bypassed.
    pub fn read_sector(&self, off: u64, buf: &mut [u8]) -> bool {
        let Some(f) = &self.fine else { return self.read_strict(off, buf) };
        ALIGNED.with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.len() < buf.len() + ALIGN {
                *slot = vec![0u8; (buf.len() + ALIGN).next_power_of_two()];
            }
            let shift = slot.as_ptr().align_offset(ALIGN);
            let t = &mut slot[shift..shift + buf.len()];
            if f.read_exact_at(t, off).is_err() {
                return false;
            }
            buf.copy_from_slice(t);
            true
        })
    }

    fn read_direct(&self, d: &File, off: u64, buf: &mut [u8]) -> bool {
        let a = ALIGN as u64;
        let start = off & !(a - 1);
        let end = ((off + buf.len() as u64 + a - 1) & !(a - 1)).min(self.size);
        let len = (end - start) as usize;
        ALIGNED.with(|cell| {
            let mut slot = cell.borrow_mut();
            if slot.len() < len + ALIGN {
                *slot = vec![0u8; (len + ALIGN).next_power_of_two()];
            }
            let shift = slot.as_ptr().align_offset(ALIGN);
            let t = &mut slot[shift..shift + len];
            if d.read_exact_at(t, start).is_err() {
                return false;
            }
            let from = (off - start) as usize;
            buf.copy_from_slice(&t[from..from + buf.len()]);
            true
        })
    }

    fn read_slow(&self, off: u64, buf: &mut [u8]) {
        let ss = self.sector as usize;
        let mut pos = 0usize;
        let mut bad_start: Option<u64> = None;
        while pos < buf.len() {
            let abs = off + pos as u64;
            let next = (ss - (abs as usize % ss)).min(buf.len() - pos);
            match self.file.read_exact_at(&mut buf[pos..pos + next], abs) {
                Ok(()) => {
                    if let Some(s) = bad_start.take() {
                        self.bad_ranges.lock().unwrap().push((s, abs));
                    }
                }
                Err(_) => {
                    buf[pos..pos + next].fill(0);
                    self.read_errors.fetch_add(1, Ordering::Relaxed);
                    if bad_start.is_none() {
                        bad_start = Some(abs);
                    }
                }
            }
            pos += next;
        }
        if let Some(s) = bad_start {
            self.bad_ranges.lock().unwrap().push((s, off + buf.len() as u64));
        }
    }

    pub fn read_vec(&self, off: u64, len: usize) -> Vec<u8> {
        let mut v = vec![0u8; len];
        self.read_at(off, &mut v);
        v
    }
}

thread_local! {
    static ALIGNED: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Is the device or any of its partitions mounted?
fn mounted(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    std::fs::read_to_string("/proc/self/mounts")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .any(|src| src == path || src.rsplit('/').next().is_some_and(|n| n.starts_with(name)))
}

/// View of a partition on the source. All offsets are relative to the volume.
#[derive(Clone)]
pub struct Vol {
    pub src: Arc<Source>,
    pub base: u64,
    pub len: u64,
}

impl Vol {
    pub fn new(src: Arc<Source>, base: u64, len: u64) -> Vol {
        Vol { src, base, len }
    }
    pub fn read(&self, off: u64, len: usize) -> Vec<u8> {
        self.src.read_vec(self.base + off, len)
    }
    pub fn read_into(&self, off: u64, buf: &mut [u8]) {
        self.src.read_at(self.base + off, buf)
    }
    /// Converts an offset within the volume to an absolute source offset.
    pub fn abs(&self, off: u64) -> u64 {
        self.base + off
    }
}

// Small bounds-checked readers: corrupt or malicious metadata returns zero
// instead of causing a panic.
#[inline]
pub fn u8_at(b: &[u8], o: usize) -> u8 {
    b.get(o).copied().unwrap_or(0)
}
#[inline]
pub fn u16le(b: &[u8], o: usize) -> u16 {
    match b.get(o..o + 2) {
        Some(s) => u16::from_le_bytes([s[0], s[1]]),
        None => 0,
    }
}
#[inline]
pub fn u32le(b: &[u8], o: usize) -> u32 {
    match b.get(o..o + 4) {
        Some(s) => u32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        None => 0,
    }
}
#[inline]
pub fn u64le(b: &[u8], o: usize) -> u64 {
    match b.get(o..o + 8) {
        Some(s) => u64::from_le_bytes(s.try_into().unwrap()),
        None => 0,
    }
}
#[inline]
pub fn u16be(b: &[u8], o: usize) -> u16 {
    match b.get(o..o + 2) {
        Some(s) => u16::from_be_bytes([s[0], s[1]]),
        None => 0,
    }
}
#[inline]
pub fn u32be(b: &[u8], o: usize) -> u32 {
    match b.get(o..o + 4) {
        Some(s) => u32::from_be_bytes([s[0], s[1], s[2], s[3]]),
        None => 0,
    }
}
#[inline]
pub fn u64be(b: &[u8], o: usize) -> u64 {
    match b.get(o..o + 8) {
        Some(s) => u64::from_be_bytes(s.try_into().unwrap()),
        None => 0,
    }
}

pub fn utf16le_string(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}
