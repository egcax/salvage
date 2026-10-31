//! ext2/3/4. Current kernels zero the extent tree and directory entry of a deleted
//! file, so the main source is the old block copies in the jbd2 journal: the
//! inode table and directory blocks are read from there as they were before deletion.

use crate::ctx::*;
use crate::model::*;
use crate::source::*;
use std::collections::{HashMap, HashSet};

const JBD2_MAGIC: u32 = 0xC03B_3998;
const EXTENTS_FL: u32 = 0x8_0000;
const INLINE_FL: u32 = 0x1000_0000;
const ROOT_INO: u32 = 2;

pub fn detect(sb: &[u8]) -> bool {
    u16le(sb, 0x38) == 0xEF53 && u32le(sb, 24) <= 6
}

struct Gd {
    block_bitmap: u64,
    inode_table: u64,
    flags: u16,
    unused: u32,
}

struct Ext<'a> {
    v: &'a Vol,
    bs: u64,
    ipg: u32,
    bpg: u32,
    isize: u32,
    first_data: u32,
    inodes: u32,
    groups: Vec<Gd>,
    jmap: HashMap<u64, Vec<(u32, u64)>>, // fs block -> (seq, journal copy offset), newest first
}

#[derive(Clone)]
struct Ino {
    mode: u16,
    links: u16,
    size: u64,
    flags: u32,
    gen: u32,
    ctime: i64,
    mtime: i64,
    dtime: i64,
    iblock: [u8; 60],
}

fn parse_inode(b: &[u8]) -> Ino {
    let mut iblock = [0u8; 60];
    if let Some(s) = b.get(40..100) {
        iblock.copy_from_slice(s);
    }
    Ino {
        mode: u16le(b, 0),
        links: u16le(b, 26),
        size: u32le(b, 4) as u64 | ((u32le(b, 108) as u64) << 32),
        flags: u32le(b, 32),
        gen: u32le(b, 100),
        ctime: u32le(b, 12) as i64,
        mtime: u32le(b, 16) as i64,
        dtime: u32le(b, 20) as i64,
        iblock,
    }
}

impl<'a> Ext<'a> {
    fn block(&self, n: u64) -> Vec<u8> {
        self.v.read(n * self.bs, self.bs as usize)
    }

    fn inode_loc(&self, ino: u32) -> Option<(u64, usize)> {
        let g = ((ino - 1) / self.ipg) as usize;
        let idx = ((ino - 1) % self.ipg) as u64;
        let gd = self.groups.get(g)?;
        let off = idx * self.isize as u64;
        Some((gd.inode_table + off / self.bs, (off % self.bs) as usize))
    }

    fn read_inode(&self, ino: u32) -> Option<Ino> {
        let (blk, off) = self.inode_loc(ino)?;
        Some(parse_inode(&self.block(blk)[off..]))
    }

    /// Finds the fullest form of a block as an extent node: the copy on disk may
    /// have been zeroed during deletion, so the older journal copy is preferred.
    fn best_extent_block(&self, n: u64) -> Vec<u8> {
        if let Some(list) = self.jmap.get(&n) {
            let mut best: Option<Vec<u8>> = None;
            let mut most = 0;
            for (_, off) in list {
                let b = self.v.read(*off, self.bs as usize);
                if u16le(&b, 0) == 0xF30A && u16le(&b, 2) > most {
                    most = u16le(&b, 2);
                    best = Some(b);
                }
            }
            if let Some(b) = best {
                return b;
            }
        }
        self.block(n)
    }

    fn extents(&self, node: &[u8], depth_guard: u32, out: &mut Vec<(u64, u64, u64, bool)>) {
        if u16le(node, 0) != 0xF30A || depth_guard > 6 {
            return;
        }
        let entries = u16le(node, 2) as usize;
        let depth = u16le(node, 6);
        for i in 0..entries.min(340) {
            let e = 12 + i * 12;
            if depth == 0 {
                let lblock = u32le(node, e) as u64;
                let mut len = u16le(node, e + 4) as u64;
                let unwritten = len > 32768;
                if unwritten {
                    len -= 32768;
                }
                let phys = ((u16le(node, e + 6) as u64) << 32) | u32le(node, e + 8) as u64;
                out.push((lblock, phys, len, unwritten));
            } else {
                let leaf = ((u16le(node, e + 8) as u64) << 32) | u32le(node, e + 4) as u64;
                let b = self.best_extent_block(leaf);
                self.extents(&b, depth_guard + 1, out);
            }
        }
    }

    fn indirect(&self, blk: u64, level: u32, lblock: &mut u64, out: &mut Vec<(u64, u64, u64, bool)>) {
        let per = self.bs / 4;
        if blk == 0 {
            *lblock += per.pow(level);
            return;
        }
        let b = self.block(blk);
        for i in 0..per as usize {
            let p = u32le(&b, i * 4) as u64;
            if level == 1 {
                if p != 0 {
                    out.push((*lblock, p, 1, false));
                }
                *lblock += 1;
            } else {
                self.indirect(p, level - 1, lblock, out);
            }
        }
    }

    fn block_map(&self, ino: &Ino) -> Vec<(u64, u64, u64, bool)> {
        let mut out = Vec::new();
        if ino.flags & EXTENTS_FL != 0 {
            self.extents(&ino.iblock, 0, &mut out);
        } else {
            let mut l = 0u64;
            for i in 0..12 {
                let p = u32le(&ino.iblock, i * 4) as u64;
                if p != 0 {
                    out.push((l, p, 1, false));
                }
                l += 1;
            }
            for (i, lvl) in [(12usize, 1u32), (13, 2), (14, 3)] {
                let p = u32le(&ino.iblock, i * 4) as u64;
                self.indirect(p, lvl, &mut l, &mut out);
            }
        }
        out.sort_by_key(|e| e.0);
        out
    }

    fn pieces(&self, ino: &Ino) -> Vec<Piece> {
        if ino.flags & INLINE_FL != 0 && ino.size <= 60 {
            return vec![Piece::Inline { data: ino.iblock[..ino.size as usize].to_vec() }];
        }
        let map = self.block_map(ino);
        let mut out: Vec<Piece> = Vec::new();
        let mut pos = 0u64;
        for (l, p, len, unwritten) in map {
            let lb = l * self.bs;
            if lb >= ino.size {
                break;
            }
            if lb > pos {
                out.push(Piece::Zero { len: lb - pos });
                pos = lb;
            }
            let bytes = (len * self.bs).min(ino.size - pos);
            if unwritten {
                out.push(Piece::Zero { len: bytes });
            } else {
                let off = self.v.abs(p * self.bs);
                match out.last_mut() {
                    Some(Piece::Data { off: o, len }) if *o + *len == off => *len += bytes,
                    _ => out.push(Piece::Data { off, len: bytes }),
                }
            }
            pos += bytes;
        }
        if pos < ino.size && !out.is_empty() {
            out.push(Piece::Zero { len: ino.size - pos });
        }
        out
    }

    /// Scans the jbd2 journal from start to end. As it is a ring buffer, old
    /// transactions stay in place even after a clean unmount.
    fn load_journal(&mut self, jino: u32) -> Result<(usize, u32), String> {
        let ino = self.read_inode(jino).ok_or("no journal inode")?;
        let map = self.block_map(&ino);
        let mut jblocks: Vec<u64> = Vec::new();
        for (l, p, len, _) in map {
            while (jblocks.len() as u64) < l {
                jblocks.push(0);
            }
            for k in 0..len {
                jblocks.push(p + k);
            }
        }
        if jblocks.is_empty() {
            return Err("journal is empty".into());
        }
        let sb = self.block(jblocks[0]);
        if u32be(&sb, 0) != JBD2_MAGIC {
            return Err("invalid journal superblock".into());
        }
        let first = u32be(&sb, 20).max(1) as usize;
        let maxlen = (u32be(&sb, 16) as usize).min(jblocks.len());
        let incompat = u32be(&sb, 40);
        let is64 = incompat & 2 != 0;
        let csum2 = incompat & 8 != 0;
        let csum3 = incompat & 0x10 != 0;
        let tag_bytes = if csum3 {
            16
        } else {
            let sz = 12 + if csum2 { 2 } else { 0 };
            if is64 {
                sz
            } else {
                sz - 4
            }
        };
        let mut raw = vec![0u8; self.bs as usize * maxlen];
        for (i, b) in jblocks.iter().take(maxlen).enumerate() {
            if *b != 0 {
                let s = i * self.bs as usize;
                self.v.read_into(*b * self.bs, &mut raw[s..s + self.bs as usize]);
            }
        }
        let bsz = self.bs as usize;
        let mut txs = 0usize;
        let mut pairs: Vec<(u64, u32, u64)> = Vec::new();
        let mut j = first;
        while j < maxlen {
            let b = &raw[j * bsz..(j + 1) * bsz];
            if u32be(b, 0) != JBD2_MAGIC || u32be(b, 4) != 1 {
                j += 1;
                continue;
            }
            let seq = u32be(b, 8);
            txs += 1;
            let mut p = 12;
            let mut d = j + 1;
            loop {
                if p + tag_bytes > bsz {
                    break;
                }
                let lo = u32be(b, p) as u64;
                let (flags, hi) = if csum3 {
                    (u32be(b, p + 4), u32be(b, p + 8) as u64)
                } else {
                    (u16be(b, p + 6) as u32, if is64 { u32be(b, p + 8) as u64 } else { 0 })
                };
                let fsblk = lo | (hi << 32);
                if d >= maxlen {
                    d = first + (d - maxlen);
                }
                if d < maxlen && jblocks[d] != 0 {
                    pairs.push((fsblk, seq, jblocks[d] * self.bs));
                }
                d += 1;
                p += tag_bytes;
                if flags & 2 == 0 {
                    p += 16;
                }
                if flags & 8 != 0 {
                    break;
                }
            }
            j = if d > j { d } else { j + 1 };
        }
        pairs.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        for (blk, seq, off) in pairs {
            self.jmap.entry(blk).or_default().push((seq, off));
        }
        Ok((txs, u32be(&sb, 24)))
    }
}

struct NameRec {
    parent: u32,
    name: String,
    deleted: bool,
}

fn valid_name(b: &[u8]) -> bool {
    !b.is_empty() && b.iter().all(|c| *c >= 0x20 && *c != b'/' && *c != 0x7F) && std::str::from_utf8(b).is_ok()
}

/// Extracts the entries of a directory block and deleted entries left in its gaps.
fn parse_dirblock(b: &[u8], inodes: u32, out: &mut Vec<(u32, String, bool)>) -> Option<u32> {
    let bs = b.len();
    let mut pos = 0usize;
    let mut self_ino = None;
    while pos + 8 <= bs {
        let ino = u32le(b, pos);
        let rec = u16le(b, pos + 4) as usize;
        let nl = b[pos + 6] as usize;
        if rec < 8 || pos + rec > bs || rec % 4 != 0 {
            return self_ino;
        }
        if nl > 0 && 8 + nl <= rec {
            let nm = &b[pos + 8..pos + 8 + nl];
            if nm == b"." {
                self_ino = Some(ino);
            } else if nm != b".." && ino != 0 && ino <= inodes && valid_name(nm) {
                out.push((ino, String::from_utf8_lossy(nm).into_owned(), false));
            }
        }
        let real = (8 + nl + 3) & !3;
        let mut p = pos + real;
        while p + 8 <= pos + rec {
            let i2 = u32le(b, p);
            let r2 = u16le(b, p + 4) as usize;
            let n2 = b[p + 6] as usize;
            let t2 = b[p + 7];
            let plausible = i2 != 0
                && i2 <= inodes
                && n2 > 0
                && (1..=7).contains(&t2)
                && p + 8 + n2 <= pos + rec
                && r2 >= 8 + n2
                && r2 % 4 == 0;
            if plausible {
                let nm = &b[p + 8..p + 8 + n2];
                if valid_name(nm) && nm != b"." && nm != b".." {
                    out.push((i2, String::from_utf8_lossy(nm).into_owned(), true));
                    p += (8 + n2 + 3) & !3;
                    continue;
                }
            }
            p += 4;
        }
        pos += rec;
    }
    self_ino
}

pub fn scan(v: &Vol, ctx: &Ctx, vol: u16, root: u32) -> Result<FsResult, String> {
    let sb = v.read(1024, 1024);
    let bs = 1024u64 << u32le(&sb, 24);
    let inodes = u32le(&sb, 0);
    let bpg = u32le(&sb, 32);
    let ipg = u32le(&sb, 40);
    let isize = if u32le(&sb, 76) >= 1 { u16le(&sb, 88) as u32 } else { 128 };
    let compat = u32le(&sb, 92);
    let incompat = u32le(&sb, 96);
    let ro = u32le(&sb, 100);
    let is64 = incompat & 0x80 != 0;
    let dsz = if is64 { (u16le(&sb, 254) as usize).max(32) } else { 32 };
    let blocks = u32le(&sb, 4) as u64 | if is64 { (u32le(&sb, 336) as u64) << 32 } else { 0 };
    let first_data = u32le(&sb, 20);
    if ipg == 0 || bpg == 0 || isize == 0 {
        return Err("invalid ext superblock".into());
    }
    let ngroups = (blocks - first_data as u64).div_ceil(bpg as u64) as usize;
    let gdt_off = (first_data as u64 + 1) * bs;
    let gdt = v.read(gdt_off, ngroups * dsz);
    let csummed = ro & 0x10 != 0 || ro & 0x400 != 0;
    let mut groups = Vec::with_capacity(ngroups);
    for g in gdt.chunks_exact(dsz) {
        let hi = |o: usize| if is64 && dsz >= 64 { (u32le(g, o) as u64) << 32 } else { 0 };
        groups.push(Gd {
            block_bitmap: u32le(g, 0) as u64 | hi(32),
            inode_table: u32le(g, 8) as u64 | hi(40),
            flags: u16le(g, 18),
            unused: if csummed { u16le(g, 28) as u32 } else { 0 },
        });
    }
    let label = String::from_utf8_lossy(&sb[120..136]).trim_end_matches('\0').to_string();
    let mut x = Ext { v, bs, ipg, bpg, isize, first_data, inodes, groups, jmap: HashMap::new() };
    let fstype = if incompat & 0x40 != 0 {
        "ext4"
    } else if compat & 4 != 0 {
        "ext3"
    } else {
        "ext2"
    };

    // Used blocks.
    ctx.set_phase("phase.bitmap", ngroups as u64);
    let mut alloc = Intervals::default();
    for (gi, g) in x.groups.iter().enumerate() {
        if gi % 64 == 0 && !ctx.alive() {
            break;
        }
        let gstart = first_data as u64 + gi as u64 * bpg as u64;
        if g.flags & 2 != 0 {
            continue; // BLOCK_UNINIT: completely empty
        }
        let bm = x.block(g.block_bitmap);
        let mut i = 0u64;
        while i < bpg as u64 {
            if bm[(i / 8) as usize] & (1 << (i % 8)) != 0 {
                let s = i;
                while i < bpg as u64 && bm[(i / 8) as usize] & (1 << (i % 8)) != 0 {
                    i += 1;
                }
                alloc.push(v.abs((gstart + s) * bs), v.abs((gstart + i) * bs));
            } else {
                i += 1;
            }
        }
        ctx.advance(gi as u64);
    }
    alloc.push(v.abs(0), v.abs((first_data as u64 + 1) * bs + (ngroups * dsz) as u64));
    alloc.normalize();

    // Journal: if it does not parse, the scan continues, only names and extents are lost.
    let mut note = None;
    let jino = u32le(&sb, 224);
    let mut jinfo = false;
    if compat & 4 != 0 && jino != 0 {
        ctx.set_phase("phase.journal", 0);
        match x.load_journal(jino) {
            Ok((txs, _)) => {
                jinfo = true;
                ctx.log(format!("ext journal: {txs} transactions, {} block copies", x.jmap.len()));
            }
            Err(e) => ctx.warn(format!("cannot read journal: {e}")),
        }
    }
    if !jinfo && fstype != "ext2" {
        note = Some("note.no_journal".to_string());
    }

    // inode tables: live directories, deleted candidates.
    let total_inodes = (x.groups.len() as u64) * ipg as u64;
    ctx.set_phase("phase.inodes", total_inodes);
    let mut names: HashMap<u32, NameRec> = HashMap::new();
    let mut deleted: Vec<(u32, Ino)> = Vec::new();
    let mut live_files: Vec<(u32, Ino)> = Vec::new();
    let mut live_dirs: HashSet<u32> = HashSet::new();
    let per_block = (bs / isize as u64) as u32;
    let mut scanned = 0u64;
    let mut dbuf: Vec<(u32, String, bool)> = Vec::new();
    for gi in 0..x.groups.len() {
        if !ctx.alive() {
            break;
        }
        let (tbl, flags, unused) = {
            let g = &x.groups[gi];
            (g.inode_table, g.flags, g.unused)
        };
        scanned += ipg as u64;
        if flags & 1 != 0 {
            ctx.advance(scanned);
            continue; // INODE_UNINIT
        }
        let used = ipg.saturating_sub(unused);
        let tbl_blocks = (used as u64 * isize as u64).div_ceil(bs);
        let tb = v.read(tbl * bs, (tbl_blocks * bs) as usize);
        for i in 0..used {
            let ino_no = gi as u32 * ipg + i + 1;
            let off = i as usize * isize as usize;
            let Some(raw) = tb.get(off..off + isize as usize) else { break };
            let inode = parse_inode(raw);
            if inode.mode == 0 {
                continue;
            }
            let ftype = inode.mode & 0xF000;
            let is_live = inode.links > 0 && inode.dtime == 0;
            if ino_no < 11 && ino_no != ROOT_INO {
                continue;
            }
            if is_live {
                if ftype == 0x4000 {
                    live_dirs.insert(ino_no);
                    for (_, p, len, _) in x.block_map(&inode).into_iter().take(4096) {
                        for k in 0..len.min(4096) {
                            let b = x.block(p + k);
                            dbuf.clear();
                            let _ = parse_dirblock(&b, inodes, &mut dbuf);
                            for (child, name, hidden) in dbuf.drain(..) {
                                if hidden {
                                    names.entry(child).or_insert(NameRec { parent: ino_no, name, deleted: true });
                                } else {
                                    names.insert(child, NameRec { parent: ino_no, name, deleted: false });
                                }
                            }
                        }
                    }
                } else if ftype == 0x8000 {
                    live_files.push((ino_no, inode));
                }
            } else {
                deleted.push((ino_no, inode));
            }
        }
        ctx.advance(scanned);
    }

    // Directory blocks in the journal: the names of deleted entries are here.
    let mut jcopies: Vec<u64> = Vec::new();
    for list in x.jmap.values() {
        for (_, off) in list {
            jcopies.push(*off);
        }
    }
    jcopies.sort_unstable();
    jcopies.dedup();
    ctx.set_phase("phase.jnames", jcopies.len() as u64);
    let itable_range: Vec<(u64, u64)> = x
        .groups
        .iter()
        .map(|g| (g.inode_table, g.inode_table + (ipg as u64 * isize as u64).div_ceil(bs)))
        .collect();
    let is_itable = |blk: u64| itable_range.iter().any(|(a, b)| blk >= *a && blk < *b);
    let mut orphan_names: HashMap<u32, String> = HashMap::new();
    let mut jnames: HashMap<u32, Vec<(u32, String)>> = HashMap::new();
    let mut lists: Vec<(&u64, &Vec<(u32, u64)>)> = x.jmap.iter().collect();
    lists.sort_by_key(|(b, _)| **b);
    for (i, (blk, list)) in lists.into_iter().enumerate() {
        if i % 256 == 0 {
            ctx.advance(i as u64);
        }
        if is_itable(*blk) {
            continue;
        }
        for (_, off) in list {
            let b = v.read(*off, bs as usize);
            dbuf.clear();
            let self_ino = parse_dirblock(&b, inodes, &mut dbuf);
            for (child, name, _) in dbuf.drain(..) {
                match self_ino {
                    Some(p) if p != 0 => {
                        let l = jnames.entry(child).or_default();
                        if !l.iter().any(|(a, b)| *a == p && *b == name) {
                            l.push((p, name.clone()));
                        }
                        names.entry(child).or_insert(NameRec { parent: p, name, deleted: true });
                    }
                    _ => {
                        orphan_names.entry(child).or_insert(name);
                    }
                }
            }
        }
    }

    // Candidates: inode table copies in the journal. Even if the number was reused,
    // an i_generation difference separates the old file. On ext2 and on inodes with
    // uncleared extents, the current record can be used too.
    let mut cands: HashMap<(u32, u32), Ino> = HashMap::new();
    let table_of = |blk: u64| {
        itable_range.iter().position(|(a, b)| blk >= *a && blk < *b).map(|g| (g as u32, blk - itable_range[g].0))
    };
    let mut tlists: Vec<(u64, Vec<u64>)> = x
        .jmap
        .iter()
        .filter(|(b, _)| is_itable(**b))
        .map(|(b, l)| (*b, l.iter().map(|e| e.1).collect()))
        .collect();
    tlists.sort_by_key(|e| e.0);
    ctx.set_phase("phase.resolve", tlists.len() as u64);
    for (i, (blk, copies)) in tlists.iter().enumerate() {
        if i % 64 == 0 {
            ctx.advance(i as u64);
            if !ctx.alive() {
                break;
            }
        }
        let Some((g, rel)) = table_of(*blk) else { continue };
        let first = g * ipg + rel as u32 * per_block + 1;
        for joff in copies {
            let b = v.read(*joff, bs as usize);
            for slot in 0..per_block {
                let raw = &b[slot as usize * isize as usize..];
                let old = parse_inode(raw);
                if old.mode & 0xF000 == 0x8000 && old.links > 0 && old.dtime == 0 && old.size > 0 {
                    cands.entry((first + slot, old.gen)).or_insert(old);
                }
            }
        }
    }
    let mut dirmap: HashMap<u32, u32> = HashMap::new();
    dirmap.insert(ROOT_INO, root);
    let lost = ctx.cat.write().unwrap().child_dir(root, "$Orphans", true);
    let mut done_ino: HashSet<(u32, u32)> = HashSet::new();
    let mut list: Vec<(u32, u32, Ino, i64)> = Vec::new();
    for ((ino_no, gen), old) in cands {
        let Some(cur) = x.read_inode(ino_no) else { continue };
        let cur_live = cur.links > 0 && cur.dtime == 0 && cur.mode != 0;
        if cur_live && cur.gen == gen {
            continue;
        }
        let dtime = if cur.gen == gen { cur.dtime } else { cur.ctime };
        done_ino.insert((ino_no, gen));
        list.push((ino_no, gen, old, dtime));
    }
    for (ino_no, cur) in &deleted {
        if cur.mode & 0xF000 == 0x8000
            && cur.size > 0
            && (cur.flags & EXTENTS_FL == 0 || u16le(&cur.iblock, 2) > 0)
            && !done_ino.contains(&(*ino_no, cur.gen))
        {
            list.push((*ino_no, cur.gen, cur.clone(), cur.dtime));
        }
    }
    list.sort_by_key(|e| e.0);
    for (ino_no, _, old, dtime) in list {
        if !ctx.alive() {
            break;
        }
        let pieces = x.pieces(&old);
        if pieces.is_empty() {
            continue;
        }
        let cur_live = x.read_inode(ino_no).map(|c| c.links > 0 && c.dtime == 0).unwrap_or(false);
        // If the inode was reused, the live name belongs to the new file.
        let live_name = names.get(&ino_no).filter(|n| !n.deleted).map(|n| n.name.clone());
        let picked: Option<(u32, String)> = if cur_live {
            jnames
                .get(&ino_no)
                .and_then(|l| l.iter().find(|(_, n)| Some(n) != live_name.as_ref()).cloned())
        } else {
            names.get(&ino_no).map(|n| (n.parent, n.name.clone()))
        };
        let (dir, name) = match picked {
            Some((p, n)) => (resolve_dir(ctx, p, &names, &mut dirmap, lost, 0), n),
            None => (lost, orphan_names.get(&ino_no).cloned().unwrap_or_else(|| format!("inode_{ino_no}"))),
        };
        let mut e = Entry::new(vol, dir, name, old.size, State::Deleted, if jinfo { "ext journal" } else { "ext inode" });
        e.mtime = old.mtime;
        e.ctime = old.ctime;
        e.dtime = dtime;
        e.chance = chance_of(&pieces, &alloc);
        e.pieces = pieces;
        ctx.add(e);
    }

    for (ino_no, inode) in live_files {
        let Some(n) = names.get(&ino_no) else { continue };
        let dir = resolve_dir(ctx, n.parent, &names, &mut dirmap, lost, 0);
        let is_trash = ctx.cat.read().unwrap().in_trash(dir);
        if !is_trash && !ctx.opts.existing {
            continue;
        }
        let mut e = Entry::new(vol, dir, n.name.clone(), inode.size, State::Existing, "ext inode");
        e.mtime = inode.mtime;
        e.ctime = inode.ctime;
        e.chance = 100;
        e.pieces = x.pieces(&inode);
        ctx.add_live(e);
    }
    let _ = (x.first_data, x.bpg, live_dirs);

    Ok(FsResult { fstype: fstype.into(), label, block_size: bs as u32, alloc, note })
}

fn chance_of(p: &[Piece], alloc: &Intervals) -> u8 {
    let (mut tot, mut used) = (0u64, 0u64);
    for x in p {
        if let Piece::Data { off, len } = x {
            tot += len;
            used += alloc.overlap(*off, off + len);
        }
    }
    if tot == 0 {
        return 95;
    }
    let f = used as f64 / tot as f64;
    if f == 0.0 {
        94
    } else if f < 0.1 {
        55
    } else if f < 0.6 {
        25
    } else {
        4
    }
}

fn resolve_dir(ctx: &Ctx, ino: u32, names: &HashMap<u32, NameRec>, map: &mut HashMap<u32, u32>, lost: u32, depth: u32) -> u32 {
    if let Some(d) = map.get(&ino) {
        return *d;
    }
    if depth > 256 {
        return lost;
    }
    let id = match names.get(&ino) {
        Some(n) => {
            let up = resolve_dir(ctx, n.parent, names, map, lost, depth + 1);
            ctx.cat.write().unwrap().child_dir(up, &n.name, n.deleted)
        }
        None => lost,
    };
    map.insert(ino, id);
    id
}

