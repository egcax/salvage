//! FAT12/16/32: directory tree, deleted entries and (in deep mode) orphan folders
//! left in free clusters.

use crate::ctx::*;
use crate::model::*;
use crate::source::*;
use std::collections::HashSet;

pub fn detect(b: &[u8]) -> bool {
    if b.len() < 512 || u16le(b, 510) != 0xAA55 {
        return false;
    }
    let bps = u16le(b, 11) as u32;
    let spc = b[13] as u32;
    let reserved = u16le(b, 14);
    let nfats = b[16];
    let valid_bps = matches!(bps, 512 | 1024 | 2048 | 4096);
    if !valid_bps || spc == 0 || !spc.is_power_of_two() || reserved == 0 || nfats == 0 || nfats > 4 {
        return false;
    }
    if &b[3..11] == b"NTFS    " || &b[3..11] == b"EXFAT   " {
        return false;
    }
    let fat16 = u16le(b, 22) as u32;
    let fat32 = u32le(b, 36);
    let total = if u16le(b, 19) != 0 { u16le(b, 19) as u32 } else { u32le(b, 32) };
    (fat16 != 0 || fat32 != 0) && total != 0
}

struct Fat {
    cluster: u64,
    data_off: u64,
    count: u32,
    fat: Vec<u32>,
    bits: u8,
}

impl Fat {
    fn off(&self, c: u32) -> u64 {
        self.data_off + (c as u64 - 2) * self.cluster
    }
    fn valid(&self, c: u32) -> bool {
        c >= 2 && c < self.count + 2
    }
    fn free(&self, c: u32) -> bool {
        self.fat.get(c as usize).copied().unwrap_or(1) == 0
    }
    fn eoc(&self, v: u32) -> bool {
        match self.bits {
            12 => v >= 0xFF8,
            16 => v >= 0xFFF8,
            _ => v >= 0x0FFF_FFF8,
        }
    }
    fn chain(&self, start: u32) -> Vec<u32> {
        let mut v = Vec::new();
        let mut c = start;
        let mut guard = self.count as usize + 2;
        while self.valid(c) && guard > 0 {
            v.push(c);
            let n = self.fat.get(c as usize).copied().unwrap_or(0);
            if n == 0 || self.eoc(n) {
                break;
            }
            c = n;
            guard -= 1;
        }
        v
    }
}

fn dos_time(date: u16, time: u16) -> i64 {
    if date == 0 {
        return 0;
    }
    let y = 1980 + (date >> 9) as i64;
    let m = ((date >> 5) & 15) as i64;
    let d = (date & 31) as i64;
    let (h, mi, s) = ((time >> 11) as i64, ((time >> 5) & 63) as i64, ((time & 31) * 2) as i64);
    days_from_civil(y, m.max(1), d.max(1)) * 86400 + h * 3600 + mi * 60 + s
}

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

struct DirRec {
    name: String,
    deleted: bool,
    is_dir: bool,
    start: u32,
    size: u32,
    mtime: i64,
    ctime: i64,
}

fn short_name(e: &[u8]) -> String {
    let mut base: Vec<u8> = e[0..8].to_vec();
    if base[0] == 0xE5 {
        base[0] = b'_';
    }
    if base[0] == 0x05 {
        base[0] = 0xE5;
    }
    let trim = |v: &[u8]| {
        let end = v.iter().rposition(|c| *c != b' ').map(|p| p + 1).unwrap_or(0);
        v[..end].iter().map(|c| *c as char).collect::<String>()
    };
    let lower_base = e[12] & 0x08 != 0;
    let lower_ext = e[12] & 0x10 != 0;
    let mut b = trim(&base);
    let mut x = trim(&e[8..11]);
    if lower_base {
        b = b.to_lowercase();
    }
    if lower_ext {
        x = x.to_lowercase();
    }
    if x.is_empty() {
        b
    } else {
        format!("{b}.{x}")
    }
}

fn parse_dir(buf: &[u8], fat32: bool) -> Vec<DirRec> {
    let mut out = Vec::new();
    let mut lfn: Vec<(u8, Vec<u16>)> = Vec::new();
    for e in buf.chunks_exact(32) {
        if e[0] == 0 {
            break;
        }
        let attr = e[11];
        if attr & 0x3F == 0x0F {
            let mut u: Vec<u16> = Vec::with_capacity(13);
            for r in [1..11usize, 14..26, 28..32] {
                for c in e[r].chunks_exact(2) {
                    u.push(u16::from_le_bytes([c[0], c[1]]));
                }
            }
            lfn.push((e[0], u));
            continue;
        }
        let deleted = e[0] == 0xE5;
        if attr & 0x08 != 0 {
            lfn.clear();
            continue;
        }
        let sn_valid = e[1..11].iter().all(|c| *c >= 0x20 && *c != 0x7F);
        if !sn_valid {
            lfn.clear();
            continue;
        }
        let mut name = short_name(e);
        if !lfn.is_empty() {
            // LFN entries are in reverse order; in deleted ones the sequence byte is lost, but
            // the physical order is kept.
            let mut units: Vec<u16> = Vec::new();
            for (_, u) in lfn.iter().rev() {
                units.extend_from_slice(u);
            }
            if let Some(p) = units.iter().position(|c| *c == 0) {
                units.truncate(p);
            }
            while units.last() == Some(&0xFFFF) {
                units.pop();
            }
            let s = String::from_utf16_lossy(&units);
            if !s.is_empty() {
                name = s;
            }
            lfn.clear();
        }
        if name == "." || name == ".." || name.is_empty() {
            continue;
        }
        let hi = if fat32 { u16le(e, 20) as u32 } else { 0 };
        let start = (hi << 16) | u16le(e, 26) as u32;
        out.push(DirRec {
            name,
            deleted,
            is_dir: attr & 0x10 != 0,
            start,
            size: u32le(e, 28),
            mtime: dos_time(u16le(e, 24), u16le(e, 22)),
            ctime: dos_time(u16le(e, 16), u16le(e, 14)),
        });
    }
    out
}

/// Clusters of a deleted file: the chain is lost, consecutive free clusters are assumed.
fn deleted_pieces(f: &Fat, v: &Vol, start: u32, size: u64) -> (Vec<Piece>, u8) {
    if !f.valid(start) {
        return (vec![], 0);
    }
    if !f.free(start) {
        return (vec![], 3);
    }
    let need = size.div_ceil(f.cluster);
    let mut left = size;
    let mut pieces: Vec<Piece> = Vec::new();
    let mut c = start;
    let mut got = 0u64;
    let mut skipped = 0u64;
    while got < need && f.valid(c) {
        if f.free(c) {
            let take = left.min(f.cluster);
            let off = v.abs(f.off(c));
            match pieces.last_mut() {
                Some(Piece::Data { off: o, len }) if *o + *len == off => *len += take,
                _ => pieces.push(Piece::Data { off, len: take }),
            }
            left -= take;
            got += 1;
        } else {
            skipped += 1;
        }
        c += 1;
    }
    let chance = if got < need {
        15
    } else if skipped == 0 {
        92
    } else {
        60
    };
    (pieces, chance)
}

pub fn scan(v: &Vol, ctx: &Ctx, vol: u16, root: u32) -> Result<FsResult, String> {
    let b = v.read(0, 4096);
    let bps = u16le(&b, 11) as u64;
    let spc = b[13] as u64;
    let reserved = u16le(&b, 14) as u64;
    let nfats = b[16] as u64;
    let root_ents = u16le(&b, 17) as u64;
    let total = if u16le(&b, 19) != 0 { u16le(&b, 19) as u64 } else { u32le(&b, 32) as u64 };
    let fatsz = if u16le(&b, 22) != 0 { u16le(&b, 22) as u64 } else { u32le(&b, 36) as u64 };
    let root_secs = (root_ents * 32).div_ceil(bps);
    let data_sec = reserved + nfats * fatsz + root_secs;
    let count = total.saturating_sub(data_sec) / spc;
    let bits = if count < 4085 {
        12
    } else if count < 65525 {
        16
    } else {
        32
    };
    let fat_off = reserved * bps;
    let cluster = spc * bps;
    let raw = v.read(fat_off, (fatsz * bps) as usize);
    let maxc = count as usize + 2;
    let mut table = vec![0u32; maxc];
    for (i, slot) in table.iter_mut().enumerate() {
        *slot = match bits {
            12 => {
                let o = i + i / 2;
                let w = u16le(&raw, o) as u32;
                if i & 1 == 1 {
                    w >> 4
                } else {
                    w & 0xFFF
                }
            }
            16 => u16le(&raw, i * 2) as u32,
            _ => u32le(&raw, i * 4) & 0x0FFF_FFFF,
        };
    }
    let f = Fat { cluster, data_off: data_sec * bps, count: count as u32, fat: table, bits };

    let mut label = String::new();
    let lab_off = if bits == 32 { 71 } else { 43 };
    if let Some(l) = b.get(lab_off..lab_off + 11) {
        let s: String = l.iter().map(|c| *c as char).collect();
        let s = s.trim().to_string();
        if s != "NO NAME" {
            label = s;
        }
    }

    let mut alloc = Intervals::default();
    alloc.push(v.abs(0), v.abs(f.data_off));
    ctx.mark(v.abs(0), v.abs(f.data_off), CELL_META);
    for c in 2..(count as u32 + 2) {
        if !f.free(c) {
            alloc.push(v.abs(f.off(c)), v.abs(f.off(c) + cluster));
        }
    }
    alloc.normalize();

    ctx.set_phase("phase.dirs", count);
    let mut seen: HashSet<u32> = HashSet::new();
    let mut stack: Vec<(Vec<u8>, u32, bool)> = Vec::new();
    let root_buf = if bits == 32 {
        let rc = u32le(&b, 44);
        seen.insert(rc);
        read_chain(&f, v, rc)
    } else {
        v.read((reserved + nfats * fatsz) * bps, (root_ents * 32) as usize)
    };
    stack.push((root_buf, root, false));
    let mut done = 0u64;
    while let Some((buf, dir, under_deleted)) = stack.pop() {
        if !ctx.alive() {
            break;
        }
        for r in parse_dir(&buf, bits == 32) {
            let deleted = r.deleted || under_deleted;
            if r.is_dir {
                if !f.valid(r.start) || !seen.insert(r.start) {
                    continue;
                }
                let child = ctx.cat.write().unwrap().child_dir(dir, &r.name, deleted);
                let dbuf = if r.deleted || under_deleted && f.free(r.start) {
                    read_deleted_dir(&f, v, r.start)
                } else {
                    read_chain(&f, v, r.start)
                };
                done += 1;
                ctx.advance(done);
                stack.push((dbuf, child, deleted));
            } else {
                add_file(&f, v, ctx, vol, dir, &r, deleted, "FAT");
            }
        }
    }

    if ctx.opts.mode != "quick" && ctx.alive() {
        orphan_sweep(&f, v, ctx, vol, root, &mut seen);
    }

    Ok(FsResult {
        fstype: format!("FAT{bits}"),
        label,
        block_size: cluster as u32,
        alloc,
        note: None,
    })
}

fn add_file(f: &Fat, v: &Vol, ctx: &Ctx, vol: u16, dir: u32, r: &DirRec, deleted: bool, method: &str) {
    let size = r.size as u64;
    if size == 0 {
        return;
    }
    let mut e = Entry::new(
        vol,
        dir,
        r.name.clone(),
        size,
        if deleted { State::Deleted } else { State::Existing },
        method,
    );
    e.mtime = r.mtime;
    e.ctime = r.ctime;
    if deleted {
        let (p, c) = deleted_pieces(f, v, r.start, size);
        e.pieces = p;
        e.chance = c;
    } else {
        let mut left = size;
        for c in f.chain(r.start) {
            let take = left.min(f.cluster);
            let off = v.abs(f.off(c));
            match e.pieces.last_mut() {
                Some(Piece::Data { off: o, len }) if *o + *len == off => *len += take,
                _ => e.pieces.push(Piece::Data { off, len: take }),
            }
            left -= take;
            if left == 0 {
                break;
            }
        }
        e.chance = 100;
        ctx.add_live(e);
        return;
    }
    ctx.add(e);
}

fn read_chain(f: &Fat, v: &Vol, start: u32) -> Vec<u8> {
    let mut out = Vec::new();
    for c in f.chain(start).into_iter().take(65536) {
        out.extend_from_slice(&v.read(f.off(c), f.cluster as usize));
    }
    out
}

fn looks_like_dir(buf: &[u8]) -> bool {
    let mut ok = 0;
    for e in buf.chunks_exact(32).take(16) {
        if e[0] == 0 {
            break;
        }
        let attr = e[11];
        if attr & 0xC0 != 0 {
            return false;
        }
        if attr & 0x3F != 0x0F && e[1..11].iter().any(|c| *c < 0x20) {
            return false;
        }
        ok += 1;
    }
    ok > 0
}

fn read_deleted_dir(f: &Fat, v: &Vol, start: u32) -> Vec<u8> {
    let mut out = Vec::new();
    let mut c = start;
    for _ in 0..256 {
        if !f.valid(c) || !f.free(c) {
            break;
        }
        let buf = v.read(f.off(c), f.cluster as usize);
        if !looks_like_dir(&buf) {
            break;
        }
        let end = buf.chunks_exact(32).any(|e| e[0] == 0);
        out.extend_from_slice(&buf);
        if end {
            break;
        }
        c += 1;
    }
    out
}

/// After a format the root can be empty, but subfolders stay in free clusters:
/// looks for clusters starting with "." and "..".
fn orphan_sweep(f: &Fat, v: &Vol, ctx: &Ctx, vol: u16, root: u32, seen: &mut HashSet<u32>) {
    ctx.set_phase("phase.orphans", f.count as u64);
    let orphan = ctx.cat.write().unwrap().child_dir(root, "$Orphans", true);
    // Bulk read: one read per cluster is too slow with direct I/O.
    const BATCH: u32 = 256;
    let mut buf = vec![0u8; (BATCH as u64 * f.cluster) as usize];
    let mut start = 2u32;
    while start < f.count + 2 {
        if !ctx.alive() {
            return;
        }
        ctx.advance(start as u64);
        let n = BATCH.min(f.count + 2 - start);
        if (start..start + n).all(|c| !f.free(c)) {
            start += n;
            continue;
        }
        let bytes = (n as u64 * f.cluster) as usize;
        v.read_into(f.off(start), &mut buf[..bytes]);
        for i in 0..n {
            let c = start + i;
            if !f.free(c) || seen.contains(&c) {
                continue;
            }
            let head = &buf[(i as u64 * f.cluster) as usize..];
            if &head[0..11] != b".          " || &head[32..43] != b"..         " || head[11] & 0x10 == 0 {
                continue;
            }
            let self_c = ((u16le(head, 20) as u32) << 16) | u16le(head, 26) as u32;
            if f.bits == 32 && self_c != c {
                continue;
            }
            seen.insert(c);
            let dbuf = read_deleted_dir(f, v, c);
            let name = format!("dir_{c}");
            let d = ctx.cat.write().unwrap().child_dir(orphan, &name, true);
            for r in parse_dir(&dbuf, f.bits == 32) {
                if r.is_dir {
                    let child = ctx.cat.write().unwrap().child_dir(d, &r.name, true);
                    if f.valid(r.start) && seen.insert(r.start) {
                        let sub = read_deleted_dir(f, v, r.start);
                        for s in parse_dir(&sub, f.bits == 32) {
                            if !s.is_dir {
                                add_file(f, v, ctx, vol, child, &s, true, "FAT orphan");
                            }
                        }
                    }
                } else {
                    add_file(f, v, ctx, vol, d, &r, true, "FAT orphan");
                }
            }
        }
        start += n;
    }
}
