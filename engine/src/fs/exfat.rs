//! exFAT: deleted directory entry sets (0x05/0x40/0x41) and the allocation bitmap.

use super::fat::days_from_civil;
use crate::ctx::*;
use crate::model::*;
use crate::source::*;
use std::collections::HashSet;

pub fn detect(b: &[u8]) -> bool {
    b.len() >= 512 && &b[3..11] == b"EXFAT   " && u16le(b, 510) == 0xAA55
}

struct Ex {
    cluster: u64,
    heap: u64,
    count: u32,
    fat: Vec<u32>,
    bitmap: Vec<u8>,
}

impl Ex {
    fn off(&self, c: u32) -> u64 {
        self.heap + (c as u64 - 2) * self.cluster
    }
    fn valid(&self, c: u32) -> bool {
        c >= 2 && c < self.count + 2
    }
    fn free(&self, c: u32) -> bool {
        let i = (c - 2) as usize;
        self.bitmap.get(i / 8).map(|b| b & (1 << (i % 8)) == 0).unwrap_or(false)
    }
    fn chain(&self, start: u32, max: u64) -> Vec<u32> {
        let mut v = Vec::new();
        let mut c = start;
        while self.valid(c) && (v.len() as u64) < max {
            v.push(c);
            let n = self.fat.get(c as usize).copied().unwrap_or(0xFFFF_FFFF);
            if n == 0 || n >= 0xFFFF_FFF7 {
                break;
            }
            c = n;
        }
        v
    }
}

fn ts(t: u32) -> i64 {
    if t == 0 {
        return 0;
    }
    let y = 1980 + (t >> 25) as i64;
    let m = ((t >> 21) & 15) as i64;
    let d = ((t >> 16) & 31) as i64;
    let h = ((t >> 11) & 31) as i64;
    let mi = ((t >> 5) & 63) as i64;
    let s = ((t & 31) * 2) as i64;
    days_from_civil(y, m.max(1), d.max(1)) * 86400 + h * 3600 + mi * 60 + s
}

struct Rec {
    name: String,
    deleted: bool,
    is_dir: bool,
    first: u32,
    size: u64,
    no_chain: bool,
    mtime: i64,
    ctime: i64,
}

fn parse(buf: &[u8]) -> Vec<Rec> {
    let mut out = Vec::new();
    let n = buf.len() / 32;
    let mut i = 0;
    while i < n {
        let e = &buf[i * 32..i * 32 + 32];
        let t = e[0];
        if t == 0x85 || t == 0x05 {
            let sec = e[1] as usize;
            let deleted = t == 0x05;
            if sec >= 2 && sec <= 18 && i + sec < n {
                let s = &buf[(i + 1) * 32..(i + 2) * 32];
                let want = if deleted { 0x40 } else { 0xC0 };
                if s[0] == want {
                    let name_len = s[3] as usize;
                    let mut units: Vec<u16> = Vec::with_capacity(name_len);
                    for k in 2..=sec {
                        let ne = &buf[(i + k) * 32..(i + k + 1) * 32];
                        if ne[0] != 0x41 && ne[0] != 0xC1 {
                            break;
                        }
                        for c in ne[2..32].chunks_exact(2) {
                            units.push(u16::from_le_bytes([c[0], c[1]]));
                        }
                    }
                    units.truncate(name_len);
                    let name = String::from_utf16_lossy(&units);
                    if !name.is_empty() {
                        out.push(Rec {
                            name,
                            deleted,
                            is_dir: u16le(e, 4) & 0x10 != 0,
                            first: u32le(s, 20),
                            size: u64le(s, 24),
                            no_chain: s[1] & 2 != 0,
                            mtime: ts(u32le(e, 12)),
                            ctime: ts(u32le(e, 8)),
                        });
                    }
                    i += sec + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

fn read_clusters(x: &Ex, v: &Vol, r: &Rec) -> Vec<u8> {
    let cn = r.size.div_ceil(x.cluster).max(1);
    let mut out = Vec::new();
    if r.no_chain || r.deleted {
        for k in 0..cn.min(4096) {
            out.extend_from_slice(&v.read(x.off(r.first) + k * x.cluster, x.cluster as usize));
        }
    } else {
        for c in x.chain(r.first, 4096) {
            out.extend_from_slice(&v.read(x.off(c), x.cluster as usize));
        }
    }
    out
}

pub fn scan(v: &Vol, ctx: &Ctx, vol: u16, root: u32) -> Result<FsResult, String> {
    let b = v.read(0, 512);
    let shift = b[108] as u32;
    let cshift = b[109] as u32;
    if !(9..=12).contains(&shift) || cshift > 25 {
        return Err("invalid exFAT boot record".into());
    }
    let sector = 1u64 << shift;
    let cluster = sector << cshift;
    let fat_off = u32le(&b, 80) as u64 * sector;
    let heap = u32le(&b, 88) as u64 * sector;
    let count = u32le(&b, 92);
    let root_c = u32le(&b, 96);
    let raw = v.read(fat_off, (count as usize + 2) * 4);
    let fat: Vec<u32> = raw.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let mut x = Ex { cluster, heap, count, fat, bitmap: Vec::new() };

    // Read the root directory: the allocation bitmap and volume label are here.
    let mut root_buf = Vec::new();
    for c in x.chain(root_c, 4096) {
        root_buf.extend_from_slice(&v.read(x.off(c), cluster as usize));
    }
    let mut label = String::new();
    for e in root_buf.chunks_exact(32) {
        match e[0] {
            0x81 if x.bitmap.is_empty() => {
                let first = u32le(e, 20);
                let len = u64le(e, 24) as usize;
                x.bitmap = v.read(x.off(first), len);
            }
            0x83 => {
                let n = (e[1] as usize).min(11);
                label = utf16le_string(&e[2..2 + n * 2]);
            }
            0 => break,
            _ => {}
        }
    }
    if x.bitmap.is_empty() {
        return Err("exFAT allocation bitmap not found".into());
    }

    let mut alloc = Intervals::default();
    alloc.push(v.abs(0), v.abs(heap));
    ctx.mark(v.abs(0), v.abs(heap), CELL_META);
    for c in 2..count + 2 {
        if !x.free(c) {
            alloc.push(v.abs(x.off(c)), v.abs(x.off(c) + cluster));
        }
    }
    alloc.normalize();

    ctx.set_phase("phase.dirs", 0);
    let mut seen: HashSet<u32> = HashSet::new();
    seen.insert(root_c);
    let mut stack = vec![(root_buf, root, false)];
    let mut done = 0;
    while let Some((buf, dir, under)) = stack.pop() {
        if !ctx.alive() {
            break;
        }
        for r in parse(&buf) {
            let deleted = r.deleted || under;
            if r.is_dir {
                if !x.valid(r.first) || !seen.insert(r.first) {
                    continue;
                }
                let child = ctx.cat.write().unwrap().child_dir(dir, &r.name, deleted);
                let dbuf = read_clusters(&x, v, &r);
                done += 1;
                ctx.advance(done);
                stack.push((dbuf, child, deleted));
            } else {
                add_file(&x, v, ctx, vol, dir, &r, deleted);
            }
        }
    }
    if ctx.opts.mode != "quick" && ctx.alive() {
        // Deleted directories may have lost the link to their parent entry: look for
        // entry sets (0x85/0x05 + a valid stream) in free clusters.
        ctx.set_phase("phase.orphans", count as u64);
        let orphan = ctx.cat.write().unwrap().child_dir(root, "$Orphans", true);
        // Bulk read: one read per cluster is too slow with direct I/O.
        const BATCH: u32 = 256;
        let mut buf = vec![0u8; (BATCH as u64 * cluster) as usize];
        let mut start = 2u32;
        while start < count + 2 {
            if !ctx.alive() {
                break;
            }
            ctx.advance(start as u64);
            let n = BATCH.min(count + 2 - start);
            if (start..start + n).all(|c| !x.free(c)) {
                start += n;
                continue;
            }
            let bytes = (n as u64 * cluster) as usize;
            v.read_into(x.off(start), &mut buf[..bytes]);
            for i in 0..n {
                let c = start + i;
                if !x.free(c) || seen.contains(&c) {
                    continue;
                }
                let at = (i as u64 * cluster) as usize;
                let head = &buf[at..at + cluster as usize];
                if !matches!(head[0], 0x05 | 0x85) || !matches!(head[32], 0x40 | 0xC0) || !matches!(head[64], 0x41 | 0xC1) {
                    continue;
                }
                let recs = parse(head);
                if recs.is_empty() {
                    continue;
                }
                seen.insert(c);
                let d = ctx.cat.write().unwrap().child_dir(orphan, &format!("dir_{c}"), true);
                for r in recs {
                    if !r.is_dir {
                        add_file(&x, v, ctx, vol, d, &r, true);
                    }
                }
            }
            start += n;
        }
    }

    Ok(FsResult { fstype: "exFAT".into(), label, block_size: cluster as u32, alloc, note: None })
}

fn add_file(x: &Ex, v: &Vol, ctx: &Ctx, vol: u16, dir: u32, r: &Rec, deleted: bool) {
    if r.size == 0 || !x.valid(r.first) {
        return;
    }
    let mut e = Entry::new(
        vol,
        dir,
        r.name.clone(),
        r.size,
        if deleted { State::Deleted } else { State::Existing },
        "exFAT",
    );
    e.mtime = r.mtime;
    e.ctime = r.ctime;
    let need = r.size.div_ceil(x.cluster);
    if deleted {
        // The chain is gone; exFAT mostly writes contiguously (NoFatChain).
        let mut left = r.size;
        let mut c = r.first;
        let mut got = 0;
        let mut skipped = 0;
        let start_free = x.free(r.first);
        while got < need && x.valid(c) {
            if x.free(c) || r.no_chain {
                let take = left.min(x.cluster);
                let off = v.abs(x.off(c));
                match e.pieces.last_mut() {
                    Some(Piece::Data { off: o, len }) if *o + *len == off => *len += take,
                    _ => e.pieces.push(Piece::Data { off, len: take }),
                }
                left -= take;
                got += 1;
            } else {
                skipped += 1;
            }
            c += 1;
        }
        e.chance = if !start_free {
            3
        } else if got < need {
            15
        } else if skipped == 0 {
            93
        } else {
            60
        };
    } else {
        let mut left = r.size;
        let list: Vec<u32> = if r.no_chain {
            (0..need as u32).map(|k| r.first + k).collect()
        } else {
            x.chain(r.first, need)
        };
        for c in list {
            let take = left.min(x.cluster);
            let off = v.abs(x.off(c));
            match e.pieces.last_mut() {
                Some(Piece::Data { off: o, len }) if *o + *len == off => *len += take,
                _ => e.pieces.push(Piece::Data { off, len: take }),
            }
            left -= take;
        }
        e.chance = 100;
        ctx.add_live(e);
        return;
    }
    ctx.add(e);
}
