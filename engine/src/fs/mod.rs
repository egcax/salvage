pub mod btrfs;
pub mod exfat;
pub mod ext;
pub mod f2fs;
pub mod fat;
pub mod ntfs;
pub mod xfs;

use crate::ctx::*;
use crate::model::*;
use crate::source::*;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct Found {
    pub fstype: &'static str,
    pub offset: u64,
    pub size: u64,
    pub lost: bool,
}

/// Is there a file system at this offset?
pub fn identify(src: &Source, off: u64) -> Option<&'static str> {
    let boot = src.read_vec(off, 4096);
    if ntfs::detect(&boot) {
        return Some("ntfs");
    }
    if exfat::detect(&boot) {
        return Some("exfat");
    }
    let sb = src.read_vec(off + 1024, 1024);
    if ext::detect(&sb) {
        return Some("ext");
    }
    let bt = src.read_vec(off + 0x10000, 4096);
    if btrfs::detect(&bt) {
        return Some("btrfs");
    }
    if xfs::detect(&boot) {
        return Some("xfs");
    }
    if f2fs::detect(&src.read_vec(off + 0x400, 1024)) {
        return Some("f2fs");
    }
    if fat::detect(&boot) {
        return Some("fat");
    }
    None
}

pub fn scanner(t: &str) -> Option<FsScan> {
    Some(match t {
        "fat" => fat::scan,
        "exfat" => exfat::scan,
        "ntfs" => ntfs::scan,
        "ext" => ext::scan,
        "btrfs" => btrfs::scan,
        "xfs" => xfs::scan,
        "f2fs" => f2fs::scan,
        _ => return None,
    })
}

/// Partition list from MBR and GPT.
pub fn partitions(src: &Source) -> Vec<(u64, u64)> {
    let ss = src.sector as u64;
    let mbr = src.read_vec(0, 512);
    let mut out = Vec::new();
    if u16le(&mbr, 510) != 0xAA55 {
        return out;
    }
    // A file system in the boot sector means this is not a partition table.
    if &mbr[3..11] == b"NTFS    " || &mbr[3..11] == b"EXFAT   " || fat::detect(&mbr) {
        return out;
    }
    let mut gpt = false;
    for i in 0..4 {
        let e = 446 + i * 16;
        let ty = mbr[e + 4];
        let lba = u32le(&mbr, e + 8) as u64;
        let cnt = u32le(&mbr, e + 12) as u64;
        if ty == 0xEE {
            gpt = true;
        } else if ty != 0 && cnt != 0 {
            if ty == 5 || ty == 0x0F || ty == 0x85 {
                extended(src, lba, &mut out);
            } else {
                out.push((lba * ss, cnt * ss));
            }
        }
    }
    if gpt {
        out.clear();
        let h = src.read_vec(ss, 512);
        if &h[0..8] == b"EFI PART" {
            let tbl = u64le(&h, 72);
            let n = u32le(&h, 80).min(1024) as usize;
            let sz = u32le(&h, 84) as usize;
            let raw = src.read_vec(tbl * ss, n * sz);
            for e in raw.chunks_exact(sz) {
                let first = u64le(e, 32);
                let last = u64le(e, 40);
                if first != 0 && last >= first {
                    out.push((first * ss, (last - first + 1) * ss));
                }
            }
        }
    }
    out
}

fn extended(src: &Source, base: u64, out: &mut Vec<(u64, u64)>) {
    let ss = src.sector as u64;
    let mut cur = base;
    for _ in 0..128 {
        let b = src.read_vec(cur * ss, 512);
        if u16le(&b, 510) != 0xAA55 {
            return;
        }
        let lba = u32le(&b, 446 + 8) as u64;
        let cnt = u32le(&b, 446 + 12) as u64;
        if cnt != 0 {
            out.push(((cur + lba) * ss, cnt * ss));
        }
        let next = u32le(&b, 446 + 16 + 8) as u64;
        if next == 0 {
            return;
        }
        cur = base + next;
    }
}

fn fs_size(src: &Source, off: u64, t: &str) -> u64 {
    match t {
        "ntfs" => {
            let b = src.read_vec(off, 512);
            u64le(&b, 0x28) * u16le(&b, 11) as u64
        }
        "exfat" => {
            let b = src.read_vec(off, 512);
            u64le(&b, 72) << b[108]
        }
        "ext" => {
            let s = src.read_vec(off + 1024, 1024);
            let bs = 1024u64 << u32le(&s, 24);
            let hi = if u32le(&s, 96) & 0x80 != 0 { (u32le(&s, 336) as u64) << 32 } else { 0 };
            (u32le(&s, 4) as u64 | hi) * bs
        }
        "btrfs" => u64le(&src.read_vec(off + 0x10000, 4096), 0x70),
        "xfs" => {
            let b = src.read_vec(off, 512);
            u64be(&b, 8) * u32be(&b, 4) as u64
        }
        "f2fs" => u64le(&src.read_vec(off + 0x400, 1024), 36) * 4096,
        "fat" => {
            let b = src.read_vec(off, 512);
            let t16 = u16le(&b, 19) as u64;
            let t = if t16 != 0 { t16 } else { u32le(&b, 32) as u64 };
            t * u16le(&b, 11) as u64
        }
        _ => 0,
    }
}

/// Finds the volumes in a source: table, offset 0 if there is none, a sweep in deep mode.
pub fn discover(ctx: &Ctx) -> Vec<Found> {
    let src = &ctx.src;
    let mut out: Vec<Found> = Vec::new();
    for (off, len) in partitions(src) {
        if off >= src.size {
            continue;
        }
        if let Some(t) = identify(src, off) {
            out.push(Found { fstype: t, offset: off, size: len.min(src.size - off), lost: false });
        }
    }
    if out.is_empty() {
        if let Some(t) = identify(src, 0) {
            out.push(Found { fstype: t, offset: 0, size: src.size, lost: false });
        }
    }
    if ctx.opts.find_lost && ctx.opts.mode != "quick" {
        sweep(ctx, &mut out);
    }
    out
}

/// Deleted partition search. Looks for file system signatures in unused space,
/// which finds volumes whose table was erased or that were overlaid by a new partition.
fn sweep(ctx: &Ctx, out: &mut Vec<Found>) {
    let src: &Arc<Source> = &ctx.src;
    let step = 1u64 << 20; // partitions are almost always 1 MiB aligned
    let extent = src.size;
    ctx.set_phase("phase.lost", extent);
    let mut off = step;
    let mut known: Vec<(u64, u64)> = out.iter().map(|f| (f.offset, f.offset + f.size)).collect();
    while off + 2 * step < extent {
        if !ctx.alive() {
            return;
        }
        if let Some((_, end)) = known.iter().find(|(s, e)| off >= *s && off < *e) {
            off = (*end).max(off + step) & !(step - 1);
            continue;
        }
        if let Some(t) = identify(src, off) {
            let sz = fs_size(src, off, t);
            if sz > 8 << 20 && off + sz <= extent + step {
                out.push(Found { fstype: t, offset: off, size: sz.min(extent - off), lost: true });
                known.push((off, off + sz));
                off = (off + sz + step - 1) & !(step - 1);
                continue;
            }
        }
        off += step;
        ctx.advance(off);
    }
}
