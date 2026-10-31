//! XFS: the extent list in a deleted inode's data fork stays in place. Name and
//! size are lost; extents are read through the inode B+ tree, size and type are
//! derived from the content.

use crate::carve::sigs;
use crate::ctx::*;
use crate::model::*;
use crate::source::*;
use std::collections::HashSet;

pub fn detect(sb: &[u8]) -> bool {
    sb.len() >= 512 && &sb[0..4] == b"XFSB" && u32be(sb, 4) >= 512
}

struct Xfs<'a> {
    v: &'a Vol,
    block: u64,
    sect: u64,
    agblocks: u64,
    agcount: u32,
    isize: u64,
    inopblog: u32,
    agblklog: u32,
    v5: bool,
}

impl Xfs<'_> {
    /// Combined block number (AG + block) -> byte offset.
    fn fsb(&self, f: u64) -> u64 {
        let ag = f >> self.agblklog;
        let b = f & ((1 << self.agblklog) - 1);
        (ag * self.agblocks + b) * self.block
    }
    fn hdr(&self) -> usize {
        if self.v5 {
            56
        } else {
            16
        }
    }
    fn walk(&self, ag: u64, root: u32, depth: u32, f: &mut dyn FnMut(&[u8], usize)) {
        if root == 0 || depth > 8 {
            return;
        }
        let b = self.v.read((ag * self.agblocks + root as u64) * self.block, self.block as usize);
        let level = u16be(&b, 4);
        let n = u16be(&b, 6) as usize;
        let h = self.hdr();
        if level == 0 {
            f(&b, n);
        } else {
            // In a node, keys run to the middle of the block, pointers fill the second half.
            let maxr = (self.block as usize - h) / 8;
            for i in 0..n.min(maxr) {
                let p = u32be(&b, h + maxr * 4 + i * 4);
                self.walk(ag, p, depth + 1, f);
            }
        }
    }
}

fn extents(lit: &[u8], n: usize) -> Vec<(u64, u64, u64, bool)> {
    let mut out = Vec::new();
    for i in 0..n {
        let Some(r) = lit.get(i * 16..i * 16 + 16) else { break };
        let a = u64be(r, 0);
        let b = u64be(r, 8);
        let unwritten = a >> 63 != 0;
        let off = (a >> 9) & ((1 << 54) - 1);
        let start = ((a & 0x1FF) << 43) | (b >> 21);
        let len = b & ((1 << 21) - 1);
        out.push((off, start, len, unwritten));
    }
    out
}

pub fn scan(v: &Vol, ctx: &Ctx, vol: u16, root: u32) -> Result<FsResult, String> {
    let sb = v.read(0, 512);
    let block = u32be(&sb, 4) as u64;
    let agblocks = u32be(&sb, 84) as u64;
    let agcount = u32be(&sb, 88);
    let sect = u16be(&sb, 102) as u64;
    let isize = u16be(&sb, 104) as u64;
    let x = Xfs {
        v,
        block,
        sect,
        agblocks,
        agcount,
        isize,
        inopblog: sb[123] as u32,
        agblklog: sb[124] as u32,
        v5: u16be(&sb, 100) & 0xF == 5,
    };
    if block == 0 || agblocks == 0 || agcount == 0 || isize == 0 {
        return Err("invalid XFS superblock".into());
    }
    let label: String = sb[108..120].iter().take_while(|c| **c != 0).map(|c| *c as char).collect();

    // Free space: the bnobt in the AGF. The rest counts as used.
    ctx.set_phase("phase.usage", agcount as u64);
    let mut free = Intervals::default();
    for ag in 0..agcount as u64 {
        let agf = v.read(ag * agblocks * block + sect, sect as usize);
        if &agf[0..4] != b"XAGF" {
            continue;
        }
        let rootb = u32be(&agf, 16);
        let base = ag * agblocks;
        x.walk(ag, rootb, 0, &mut |b, n| {
            let h = x.hdr();
            for i in 0..n {
                let s = u32be(b, h + i * 8) as u64;
                let c = u32be(b, h + i * 8 + 4) as u64;
                free.push(v.abs((base + s) * block), v.abs((base + s + c) * block));
            }
        });
        ctx.advance(ag);
    }
    free.normalize();
    let mut alloc = Intervals::default();
    let mut pos = v.abs(0);
    for (a, b) in free.items() {
        alloc.push(pos, *a);
        pos = *b;
    }
    alloc.push(pos, v.abs(x.agblocks * agcount as u64 * block));
    alloc.normalize();

    // inode chunks: every inode marked free is a candidate.
    ctx.set_phase("phase.inodes", agcount as u64);
    let lost = ctx.cat.write().unwrap().child_dir(root, "$Orphans", true);
    let mut seen: HashSet<u64> = HashSet::new();
    for ag in 0..agcount as u64 {
        if !ctx.alive() {
            break;
        }
        ctx.advance(ag);
        let agi = v.read(ag * agblocks * block + 2 * sect, sect as usize);
        if &agi[0..4] != b"XAGI" {
            continue;
        }
        let mut chunks: Vec<(u32, u64)> = Vec::new();
        x.walk(ag, u32be(&agi, 20), 0, &mut |b, n| {
            let h = x.hdr();
            for i in 0..n {
                let r = h + i * 16;
                chunks.push((u32be(b, r), u64be(b, r + 8)));
            }
        });
        for (start, freemask) in chunks {
            let off = (ag * agblocks + (start as u64 >> x.inopblog)) * block;
            let raw = v.read(off, (64 * isize) as usize);
            for i in 0..64u64 {
                if freemask & (1 << i) == 0 {
                    continue;
                }
                let d = &raw[(i * isize) as usize..((i + 1) * isize) as usize];
                let ino = (ag << (x.inopblog + x.agblklog)) | (start as u64 + i);
                if &d[0..2] != b"IN" || !seen.insert(ino) {
                    continue;
                }
                let core = if d[4] >= 3 { 176 } else { 100 };
                add_deleted(&x, ctx, vol, lost, ino, d, core, &alloc);
            }
        }
    }
    Ok(FsResult { fstype: "XFS".into(), label, block_size: block as u32, alloc, note: None })
}

fn add_deleted(x: &Xfs, ctx: &Ctx, vol: u16, lost: u32, ino: u64, d: &[u8], core: usize, alloc: &Intervals) {
    // A free inode has a zero extent counter; the records stay anyway. Valid
    // extents end at the first invalid record.
    let list = extents(&d[core..], (x.isize as usize - core) / 16);
    let mut out: Vec<(u64, u64, u64)> = Vec::new();
    let total = x.agblocks * x.agcount as u64;
    let mut next = 0u64;
    for (off, start, len, unwritten) in list {
        if len == 0 || unwritten || off < next {
            break;
        }
        let at = x.fsb(start) / x.block;
        if at + len > total {
            break;
        }
        out.push((off, start, len));
        next = off + len;
    }
    if out.is_empty() {
        return;
    }
    let mut pieces: Vec<Piece> = Vec::new();
    let mut pos = 0u64;
    let (mut tot, mut used) = (0u64, 0u64);
    for (off, start, len) in &out {
        let l = off * x.block;
        if l > pos {
            pieces.push(Piece::Zero { len: l - pos });
        }
        let abs = x.v.abs(x.fsb(*start));
        let b = len * x.block;
        tot += b;
        used += alloc.overlap(abs, abs + b);
        match pieces.last_mut() {
            Some(Piece::Data { off: o, len: n }) if *o + *n == abs => *n += b,
            _ => pieces.push(Piece::Data { off: abs, len: b }),
        }
        pos = l + b;
    }
    let mut size = pos;
    // The size is gone: for a recognized single-fragment file, the real size is found.
    if let Some(Piece::Data { off, .. }) = pieces.first() {
        let head = ctx.src.read_vec(*off, 4096);
        let r = sigs::R::new(&ctx.src, *off, size);
        if let Some(h) = sigs::probe(&head, &r) {
            if h.complete && h.size <= size {
                size = h.size;
                if let Some(Piece::Data { len, .. }) = pieces.last_mut() {
                    if pieces_single(*len, size) {
                        *len = size;
                    }
                }
            }
        }
    }
    let mut e = Entry::new(vol, lost, format!("inode_{ino}"), size, State::Deleted, "XFS inode");
    e.mtime = u32be(d, 40) as i64;
    e.ctime = u32be(d, 48) as i64;
    let f = used as f64 / tot.max(1) as f64;
    e.chance = if f == 0.0 {
        85
    } else if f < 0.1 {
        50
    } else if f < 0.6 {
        25
    } else {
        4
    };
    e.pieces = pieces;
    ctx.add(e);
}

fn pieces_single(len: u64, size: u64) -> bool {
    size <= len
}
