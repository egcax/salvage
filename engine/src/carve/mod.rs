//! Signature scanner: independent of the file system, it walks free space (or the whole disk)
//! sector by sector. Single-threaded on purpose: the bottleneck is the disk itself, and
//! this way it stays easy on the battery.

pub mod repair;
pub mod sigs;

use crate::ctx::*;
use crate::model::*;
use sigs::{first_byte_table, probe, probe_tar, R};
use std::collections::HashMap;

const CHUNK: usize = 8 << 20;

pub struct Stats {
    pub scanned: u64,
    pub zero: u64,
}

/// `regions`: ranges to scan, in absolute offsets.
pub fn run(ctx: &Ctx, regions: &[(u64, u64)], vol: u16, root: u32, known: &HashMap<u64, u64>, block: u64) -> Stats {
    let total: u64 = regions.iter().map(|(a, b)| b - a).sum();
    let started = std::time::Instant::now();
    let mut last_mark = std::time::Instant::now();
    ctx.set_phase("phase.carve", total);
    ctx.src.advise_sequential();
    let table = first_byte_table();
    let step = (ctx.opts.step.max(1) as usize).min(4096);
    let want = &ctx.opts.kinds;
    let carved = ctx.cat.write().unwrap().child_dir(root, "$Carved", true);
    let mut dirs: HashMap<Kind, u32> = HashMap::new();
    let mut buf = vec![0u8; CHUNK];
    let mut stats = Stats { scanned: 0, zero: 0 };
    let mut done = 0u64;

    for (ri, &(start, end)) in regions.iter().enumerate() {
        let mut pos = start.div_ceil(step as u64) * step as u64;
        while pos < end {
            if !ctx.alive() {
                return stats;
            }
            // Resume from here if interrupted.
            if last_mark.elapsed().as_secs() >= 1 {
                last_mark = std::time::Instant::now();
                let mut p = vec![(pos, end)];
                p.extend_from_slice(&regions[ri + 1..]);
                ctx.cat.write().unwrap().pending = p;
            }
            ctx.throttle(done, started);
            let n = ((end - pos) as usize).min(CHUNK);
            ctx.src.read_at(pos, &mut buf[..n]);
            ctx.src.drop_cache(pos, n as u64);
            let mut i = 0usize;
            let mut jumped = None;
            let mut chunk_zero = 0u64;
            while i < n {
                // Skip empty 4 KiB blocks quickly and count the TRIM ratio.
                if i % 4096 == 0 && i + 4096 <= n && is_zero(&buf[i..i + 4096]) {
                    chunk_zero += 4096;
                    i += 4096;
                    continue;
                }
                let h = &buf[i..];
                let abs = pos + i as u64;
                let quick = table[h[0] as usize]
                    || (h.len() > 8 && matches!(&h[4..8], b"ftyp" | b"moov" | b"wide" | b"mdat"))
                    || (h.len() > 262 && &h[257..262] == b"ustar");
                if quick {
                    if let Some(sz) = known.get(&abs) {
                        // Do not repeat a file already found through metadata.
                        let to = abs + (*sz).max(step as u64);
                        if to < pos + n as u64 {
                            i = (to.div_ceil(step as u64) * step as u64 - pos) as usize;
                            continue;
                        }
                        jumped = Some(to);
                        break;
                    }
                    let r = R::new(&ctx.src, abs, ctx.src.size - abs);
                    let head_len = h.len().min(4096);
                    let mut local = [0u8; 4096];
                    let head: &[u8] = if head_len == 4096 {
                        &h[..4096]
                    } else {
                        ctx.src.read_at(abs, &mut local);
                        &local
                    };
                    let mut hit = probe(head, &r).or_else(|| probe_tar(head, &r));
                    // Fragmented JPEG: look for the second fragment.
                    if ctx.opts.repair {
                        if let Some(h) = hit.as_mut() {
                            // Even with an EOI, text or an empty block may have landed in between.
                            let gap = if h.ext != "jpg" {
                                None
                            } else if !h.complete {
                                Some(h.size)
                            } else {
                                repair::foreign_block(&ctx.src, abs, h.size, block)
                            };
                            if let Some(bad) = gap {
                                if let Some(parts) = repair::jpeg_bifragment(&ctx.src, abs, bad, block) {
                                    h.size = parts.iter().map(|p| p.1).sum();
                                    h.parts = Some(parts);
                                    h.complete = true;
                                    h.title = None;
                                    h.repaired = true;
                                }
                            }
                        }
                    }
                    if let Some(hit) = hit {
                        if hit.size >= 64 && hit.size >= ctx.opts.min_size.max(1) {
                            let kind = sigs::kind_of(hit.ext);
                            if want.is_empty() || want.contains(&kind) {
                                let d = *dirs.entry(kind).or_insert_with(|| {
                                    let name = dir_name(kind);
                                    ctx.cat.write().unwrap().child_dir(carved, name, true)
                                });
                                add(ctx, vol, d, abs, &hit);
                                ctx.mark(abs, abs + hit.size, CELL_FOUND);
                            }
                            // For a fragmented file, also scan the blocks in the gap: jump to the end of the first fragment.
                            let span = hit.parts.as_ref().map(|p| p[0].0 + p[0].1).unwrap_or(hit.size);
                            let to = abs + span;
                            if to < pos + n as u64 {
                                i = (to.div_ceil(step as u64) * step as u64 - pos) as usize;
                                continue;
                            }
                            jumped = Some(to);
                            break;
                        }
                    }
                }
                i += step;
            }
            stats.scanned += n as u64;
            stats.zero += chunk_zero;
            if chunk_zero >= n as u64 / 2 {
                ctx.mark(pos, pos + n as u64, CELL_ZERO);
            } else {
                ctx.mark(pos, pos + n as u64, CELL_DATA);
            }
            match jumped {
                Some(to) => {
                    let to = to.div_ceil(step as u64) * step as u64;
                    let moved = to.saturating_sub(pos).min(end - pos);
                    done += moved;
                    pos = to.max(pos + step as u64);
                }
                None => {
                    done += n as u64;
                    pos += n as u64;
                }
            }
            ctx.advance(done.min(total));
        }
    }
    stats
}

fn is_zero(b: &[u8]) -> bool {
    let (a, m, c) = unsafe { b.align_to::<u64>() };
    a.iter().all(|x| *x == 0) && m.iter().all(|x| *x == 0) && c.iter().all(|x| *x == 0)
}

fn dir_name(k: Kind) -> &'static str {
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

fn add(ctx: &Ctx, vol: u16, dir: u32, off: u64, h: &sigs::Hit) {
    let stem = match &h.title {
        Some(t) => t.replace(['/', '\0'], "_"),
        None => format!("{}_{:x}", h.ext, off),
    };
    let mut e = Entry::new(vol, dir, format!("{stem}.{}", h.ext), h.size, State::Carved, "signature");
    match &h.parts {
        Some(parts) => {
            for (o, n) in parts {
                e.pieces.push(Piece::Data { off: off + o, len: *n });
            }
        }
        None => e.pieces.push(Piece::Data { off, len: h.size }),
    }
    e.mtime = h.mtime;
    e.chance = if h.repaired {
        70
    } else if h.complete {
        86
    } else {
        38
    };
    if h.repaired {
        e.note = Some("note.repaired".into());
    } else if !h.complete {
        e.note = Some("note.truncated".into());
    }
    ctx.add(e);
}
