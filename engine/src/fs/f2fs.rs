//! F2FS: as it is log-structured, old copies of an inode stay in the main area and
//! the file name is inside the inode. Node blocks are swept, and the newest copy
//! that the NAT no longer points at is considered deleted.

use crate::ctx::*;
use crate::model::*;
use crate::source::*;
use std::collections::HashMap;

const MAGIC: u32 = 0xF2F5_2010;
const BLK: u64 = 4096;
const NULL_ADDR: u32 = 0;
const NEW_ADDR: u32 = 0xFFFF_FFFF;
const ADDRS: usize = 923;

pub fn detect(b: &[u8]) -> bool {
    b.len() >= 8 && u32le(b, 0) == MAGIC
}

#[derive(Clone)]
struct Inode {
    blk: u32,
    cp: u64,
    size: u64,
    pino: u32,
    name: String,
    mtime: i64,
    ctime: i64,
    addrs: Vec<u32>,
    links: u32,
    nids: [u32; 5],
    inline: bool,
    inline_data: Vec<u8>,
}

pub fn scan(v: &Vol, ctx: &Ctx, vol: u16, root: u32) -> Result<FsResult, String> {
    let sb = v.read(0x400, 1024);
    if !detect(&sb) {
        return Err("F2FS superblock not found".into());
    }
    if u32le(&sb, 16) != 12 {
        return Err("unsupported F2FS block size".into());
    }
    let segblk = 1u64 << u32le(&sb, 20);
    let nat = u32le(&sb, 84) as u64;
    let main = u32le(&sb, 92) as u64;
    let seg_main = u32le(&sb, 68) as u64;
    let seg_nat = u32le(&sb, 60) as u64;
    let root_ino = u32le(&sb, 96);
    let label = {
        let units: Vec<u16> = sb[0x7c..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|c| *c != 0)
            .collect();
        String::from_utf16_lossy(&units)
    };
    let total = seg_main * segblk;

    // Sweep the main area: node blocks have a footer at the end.
    ctx.set_phase("phase.inodes", total);
    let mut latest: HashMap<u32, Inode> = HashMap::new();
    let mut live_data: HashMap<u32, Inode> = HashMap::new();
    let mut indirect: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut buf = vec![0u8; (segblk * BLK) as usize];
    for seg in 0..seg_main {
        if !ctx.alive() {
            break;
        }
        ctx.advance(seg * segblk);
        let base = main + seg * segblk;
        v.read_into(base * BLK, &mut buf);
        for (i, b) in buf.chunks_exact(BLK as usize).enumerate() {
            let nid = u32le(b, 4072);
            let ino = u32le(b, 4076);
            // The top 32 bits can be a CRC: the version is in the low 32 bits.
            let cp = u64le(b, 4084) & 0xFFFF_FFFF;
            if nid == 0 || ino == 0 || cp == 0 || nid > (1 << 28) {
                continue;
            }
            let blk = base as u32 + i as u32;
            if nid == ino {
                let mode = u16le(b, 0);
                let namelen = u32le(b, 88) as usize;
                if mode & 0xF000 == 0 || namelen > 255 {
                    continue;
                }
                let flags = b[3];
                let extra = if flags & 0x20 != 0 { u16le(b, 360) as usize / 4 } else { 0 };
                if mode & 0xF000 != 0x8000 {
                    // Folder names are kept to build paths.
                    let n = String::from_utf8_lossy(&b[92..92 + namelen]).into_owned();
                    latest.entry(ino).and_modify(|c| {
                        if cp >= c.cp {
                            c.name = n.clone();
                            c.pino = u32le(b, 84);
                            c.cp = cp;
                            c.blk = blk;
                        }
                    }).or_insert(Inode {
                        blk, cp, size: 0, pino: u32le(b, 84), name: n, mtime: 0, ctime: 0,
                        addrs: vec![], links: u32le(b, 12), nids: [0; 5], inline: false, inline_data: vec![],
                    });
                    continue;
                }
                // Inline xattrs use the last 50 entries of the address area.
                let usable = ADDRS - if flags & 0x01 != 0 { 50 } else { 0 };
                let addrs: Vec<u32> = (extra..usable).map(|k| u32le(b, 360 + k * 4)).collect();
                let mut nids = [0u32; 5];
                for (k, s) in nids.iter_mut().enumerate() {
                    *s = u32le(b, 4052 + k * 4);
                }
                let inline = flags & 0x02 != 0;
                let ino_rec = Inode {
                    blk,
                    cp,
                    size: u64le(b, 16),
                    pino: u32le(b, 84),
                    name: String::from_utf8_lossy(&b[92..92 + namelen]).into_owned(),
                    mtime: u64le(b, 48) as i64,
                    ctime: u64le(b, 40) as i64,
                    links: u32le(b, 12),
                    nids,
                    inline,
                    inline_data: if inline { b[364..364 + (ADDRS - 2) * 4].to_vec() } else { vec![] },
                    addrs,
                };
                let has_data = ino_rec.size > 0 && (inline || ino_rec.addrs.iter().any(|a| *a != NULL_ADDR));
                let cur = latest.get(&ino);
                if cur.map(|c| cp >= c.cp).unwrap_or(true) {
                    latest.insert(ino, ino_rec.clone());
                }
                if has_data && live_data.get(&ino).map(|c| cp >= c.cp).unwrap_or(true) {
                    live_data.insert(ino, ino_rec);
                }
            } else {
                // Direct node: 1018 addresses.
                // Truncation writes a new zeroed version of the node: the fullest version is kept.
                let list: Vec<u32> = (0..1018).map(|k| u32le(b, k * 4)).collect();
                let used = list.iter().filter(|a| **a != NULL_ADDR).count();
                let cur = indirect.entry(nid).or_default();
                if used > cur.iter().filter(|a| **a != NULL_ADDR).count() {
                    *cur = list;
                }
            }
        }
    }

    // NAT: 455 entries per block, two copies (segment pairs).
    ctx.set_phase("phase.bitmap", 0);
    let nat_blocks = seg_nat * segblk;
    let nat_raw = v.read(nat * BLK, (nat_blocks * BLK).min(256 << 20) as usize);
    // Recent changes sit in the checkpoint journal before reaching the NAT; the
    // checkpoint bitmap also tells which copy of each NAT block is active.
    let cp = checkpoint(v, &sb, segblk);
    let nat_addr = |nid: u32| -> u32 {
        if let Some(b) = cp.journal.get(&nid) {
            return *b;
        }
        let per = 455u64;
        let blk = nid as u64 / per;
        let copy = u64::from(cp.nat_copy(blk));
        let at = (blk / segblk * 2 + copy) * segblk + blk % segblk;
        u32le(&nat_raw, (at * BLK) as usize + (nid as u64 % per) as usize * 9 + 5)
    };

    ctx.set_phase("phase.resolve", latest.len() as u64);
    let lost = ctx.cat.write().unwrap().child_dir(root, "$Orphans", true);
    let mut dirs: HashMap<u32, u32> = HashMap::new();
    dirs.insert(root_ino, root);
    let mut alloc = Intervals::default();
    alloc.push(v.abs(0), v.abs(main * BLK));
    for (ino, cur) in &latest {
        // If the NAT address points at a node, the file is live.
        let live = nat_addr(*ino) != NULL_ADDR;
        if live {
            for a in &cur.addrs {
                if *a != NULL_ADDR && *a != NEW_ADDR {
                    alloc.push(v.abs(*a as u64 * BLK), v.abs((*a as u64 + 1) * BLK));
                }
            }
            continue;
        }
        let Some(d) = live_data.get(ino) else { continue };
        let dir = dir_of(ctx, d.pino, &latest, &mut dirs, lost, 0);
        let mut e = Entry::new(vol, dir, name_of(d, *ino), d.size, State::Deleted, "F2FS inode");
        e.mtime = d.mtime;
        e.ctime = d.ctime;
        e.chance = 80;
        e.pieces = pieces(d, &indirect, v);
        if e.pieces.is_empty() {
            continue;
        }
        ctx.add(e);
    }
    alloc.normalize();
    Ok(FsResult { fstype: "F2FS".into(), label, block_size: BLK as u32, alloc, note: None })
}

struct Cp {
    journal: HashMap<u32, u32>,
    bitmap: Vec<u8>,
}

impl Cp {
    fn nat_copy(&self, blk: u64) -> bool {
        self.bitmap.get((blk / 8) as usize).is_some_and(|b| b & (0x80 >> (blk % 8)) != 0)
    }
}

fn checkpoint(v: &Vol, sb: &[u8], segblk: u64) -> Cp {
    let cpa = u32le(sb, 76) as u64;
    let c1 = v.read(cpa * BLK, BLK as usize);
    let c2 = v.read((cpa + segblk) * BLK, BLK as usize);
    let (cp, base) = if u64le(&c1, 0) >= u64le(&c2, 0) { (c1, cpa) } else { (c2, cpa + segblk) };
    let flags = u32le(&cp, 132);
    let start = u32le(&cp, 140) as u64;
    let b = v.read((base + start) * BLK, BLK as usize);
    let at = if flags & 0x4 != 0 { 0 } else { 3584 };
    let n = (u16le(&b, at) as usize).min(38);
    let mut journal = HashMap::new();
    for i in 0..n {
        let e = at + 2 + i * 13;
        journal.insert(u32le(&b, e), u32le(&b, e + 9));
    }
    let sit = u32le(&cp, 156) as usize;
    let nat = u32le(&cp, 160) as usize;
    let bitmap = cp.get(192 + sit..192 + sit + nat).map(|s| s.to_vec()).unwrap_or_default();
    Cp { journal, bitmap }
}

fn name_of(d: &Inode, ino: u32) -> String {
    if d.name.is_empty() {
        format!("inode_{ino}")
    } else {
        d.name.clone()
    }
}

fn dir_of(ctx: &Ctx, ino: u32, all: &HashMap<u32, Inode>, map: &mut HashMap<u32, u32>, lost: u32, depth: u32) -> u32 {
    if let Some(d) = map.get(&ino) {
        return *d;
    }
    if depth > 128 {
        return lost;
    }
    let id = match all.get(&ino) {
        Some(p) if !p.name.is_empty() => {
            let up = dir_of(ctx, p.pino, all, map, lost, depth + 1);
            ctx.cat.write().unwrap().child_dir(up, &p.name, true)
        }
        _ => lost,
    };
    map.insert(ino, id);
    id
}

fn pieces(d: &Inode, ind: &HashMap<u32, Vec<u32>>, v: &Vol) -> Vec<Piece> {
    if d.inline {
        let n = (d.size as usize).min(d.inline_data.len());
        return vec![Piece::Inline { data: d.inline_data[..n].to_vec() }];
    }
    let mut blocks: Vec<u32> = d.addrs.clone();
    for nid in &d.nids[..2] {
        if *nid != 0 {
            match ind.get(nid) {
                Some(l) => blocks.extend_from_slice(l),
                None => blocks.extend(std::iter::repeat_n(0, 1018)),
            }
        }
    }
    let mut out: Vec<Piece> = Vec::new();
    let mut left = d.size;
    for a in blocks {
        if left == 0 {
            break;
        }
        let take = left.min(BLK);
        if a == NULL_ADDR || a == NEW_ADDR {
            out.push(Piece::Zero { len: take });
        } else {
            let off = v.abs(a as u64 * BLK);
            match out.last_mut() {
                Some(Piece::Data { off: o, len }) if *o + *len == off => *len += take,
                _ => out.push(Piece::Data { off, len: take }),
            }
        }
        left -= take;
    }
    out
}
