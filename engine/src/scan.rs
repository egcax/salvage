//! Scan flow: find volumes, read metadata, scan free space, verify results.

use crate::carve;
use crate::carve::sigs;
use crate::ctx::*;
use crate::fs;
use crate::model::*;
use crate::reader;
use crate::source::*;
use serde_json::json;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

pub fn run(ctx: Arc<Ctx>) {
    let t0 = std::time::Instant::now();
    if ctx.opts.background {
        // IOPRIO_CLASS_IDLE: back off while other reads are running.
        unsafe { libc::syscall(libc::SYS_ioprio_set, 1, 0, (3i32 << 13) | 7) };
    }
    if ctx.opts.resume {
        resume(&ctx, t0);
        return;
    }
    let found = fs::discover(&ctx);
    let mut allocs: Vec<(u64, u64, Intervals, bool)> = Vec::new();
    let mut roots: Vec<u32> = Vec::new();

    for (i, f) in found.iter().enumerate() {
        if !ctx.alive() {
            break;
        }
        let vol = i as u16;
        let title = if f.lost { format!("Lost partition {} ({})", i + 1, f.fstype) } else { format!("Volume {}", i + 1) };
        let root = ctx.cat.write().unwrap().add_root(vol, &title);
        roots.push(root);
        let v = Vol::new(ctx.src.clone(), f.offset, f.size);
        let scanner = fs::scanner(f.fstype).expect("known type");
        ctx.log(format!("{}: {} offset {} size {}", title, f.fstype, f.offset, f.size));
        let res = catch_unwind(AssertUnwindSafe(|| scanner(&v, &ctx, vol, root)));
        let mut info = VolumeInfo {
            index: vol,
            fstype: f.fstype.to_string(),
            offset: f.offset,
            size: f.size,
            root,
            lost: f.lost,
            ..Default::default()
        };
        match res {
            Ok(Ok(r)) => {
                info.fstype = r.fstype;
                info.label = r.label;
                info.block_size = r.block_size;
                info.note = r.note;
                let used: u64 = r.alloc.items().iter().map(|(a, b)| b - a).sum();
                info.free_bytes = f.size.saturating_sub(used);
                let has = !r.alloc.items().is_empty();
                allocs.push((f.offset, f.size, r.alloc, has));
            }
            Ok(Err(e)) => {
                ctx.warn(format!("{title}: {e}"));
                info.note = Some(e);
                allocs.push((f.offset, f.size, Intervals::default(), false));
            }
            Err(_) => {
                ctx.warn(format!("{title}: metadata unreadable (damaged structures)"));
                info.note = Some("note.meta_bad".into());
                allocs.push((f.offset, f.size, Intervals::default(), false));
            }
        }
        ctx.cat.write().unwrap().volumes.push(info);
    }

    rescue(&ctx);

    if ctx.opts.mode != "quick" && ctx.alive() {
        let (start, end) = (ctx.opts.start.unwrap_or(0), ctx.opts.end.unwrap_or(ctx.src.size).min(ctx.src.size));
        let mut regions: Vec<(u64, u64)> = Vec::new();
        if ctx.opts.free_only && ctx.opts.mode != "full" {
            let mut used = Intervals::default();
            for (o, s, a, has) in &allocs {
                if *has {
                    for (x, y) in a.items() {
                        used.push(*x, *y);
                    }
                } else {
                    let _ = (o, s);
                }
            }
            used.normalize();
            regions = used.gaps(start, end);
        } else {
            regions.push((start, end));
        }
        let free: u64 = regions.iter().map(|(a, b)| b - a).sum();
        ctx.log(format!("Signature scan: {} regions, {} bytes", regions.len(), free));
        let (vol, root) = match roots.first() {
            Some(r) => (0, *r),
            None => {
                let r = ctx.cat.write().unwrap().add_root(0, "Disk");
                let info = VolumeInfo { fstype: "unknown".into(), size: ctx.src.size, root: r, ..Default::default() };
                ctx.cat.write().unwrap().volumes.push(info);
                (0, r)
            }
        };
        let known: HashMap<u64, u64> = {
            let c = ctx.cat.read().unwrap();
            c.entries
                .iter()
                .filter_map(|e| e.first_off().map(|o| (o, e.size)))
                .collect()
        };
        ctx.cat.write().unwrap().pending = regions.clone();
        carve_regions(&ctx, &regions, vol, root, &known);
    }

    finish(&ctx, t0);
}

fn carve_regions(ctx: &Ctx, regions: &[(u64, u64)], vol: u16, root: u32, known: &HashMap<u64, u64>) {
    let block = ctx.cat.read().unwrap().volumes.first().map(|v| v.block_size as u64).filter(|b| *b >= 512).unwrap_or(4096);
    let st = carve::run(ctx, regions, vol, root, known, block);
    if ctx.alive() {
        ctx.cat.write().unwrap().pending.clear();
    }
    if st.scanned > 0 {
        let ratio = st.zero as f64 / st.scanned as f64;
        for v in ctx.cat.write().unwrap().volumes.iter_mut() {
            v.zero_ratio = Some(ratio);
        }
        ctx.log(format!("{:.0}% of free space is zeroed", ratio * 100.0));
    }
}

/// Resumes an interrupted signature scan from a saved session.
fn resume(ctx: &Ctx, t0: std::time::Instant) {
    let (regions, root, known) = {
        let c = ctx.cat.read().unwrap();
        let known: HashMap<u64, u64> = c.entries.iter().filter_map(|e| e.first_off().map(|o| (o, e.size))).collect();
        (c.pending.clone(), c.volumes.first().map(|v| v.root).unwrap_or(0), known)
    };
    {
        let c = ctx.cat.read().unwrap();
        let vis = c.entries.iter().filter(|e| e.state != State::Existing);
        let (n, b) = vis.fold((0u64, 0u64), |a, e| (a.0 + 1, a.1 + e.size));
        ctx.found.store(n, std::sync::atomic::Ordering::Relaxed);
        ctx.found_bytes.store(b, std::sync::atomic::Ordering::Relaxed);
    }
    ctx.log(format!("Resuming: {} regions left", regions.len()));
    carve_regions(ctx, &regions, 0, root, &known);
    finish(ctx, t0);
}

/// Emergency rescue: copies the best candidates at once to beat TRIM.
fn rescue(ctx: &Ctx) {
    let Some(dest) = ctx.opts.rescue_dest.clone() else { return };
    if !ctx.alive() {
        return;
    }
    let limit = if ctx.opts.rescue_limit == 0 { 2 << 30 } else { ctx.opts.rescue_limit };
    let ids: Vec<u32> = {
        let c = ctx.cat.read().unwrap();
        let mut list: Vec<&Entry> = c.entries.iter().filter(|e| e.state == State::Deleted && e.chance >= 40).collect();
        list.sort_by_key(|e| std::cmp::Reverse((e.chance >= 80, e.mtime.max(e.dtime))));
        let mut total = 0u64;
        list.into_iter()
            .filter(|e| {
                if total + e.size > limit {
                    return false;
                }
                total += e.size;
                true
            })
            .map(|e| e.id)
            .collect()
    };
    if ids.is_empty() {
        return;
    }
    ctx.set_phase("phase.rescue", ids.len() as u64);
    let o = crate::recover::Opts { dest, skip_lost: true, ..Default::default() };
    crate::recover::run(ctx.src.clone(), ctx.cat.clone(), ids, o, ctx.ctl.clone(), ctx.em.clone());
}

/// Closes the scan: verification, duplicate detection, finish event.
fn finish(ctx: &Ctx, t0: std::time::Instant) {
    if ctx.alive() {
        verify(ctx);
    }
    if ctx.alive() {
        dedupe(ctx);
    }

    let cancelled = ctx.ctl.is_cancelled();
    ctx.emit_progress(true);
    let (n, b, v) = {
        let c = ctx.cat.read().unwrap();
        let visible: Vec<&Entry> = c.entries.iter().filter(|e| e.state != State::Existing).collect();
        (visible.len(), visible.iter().map(|e| e.size).sum::<u64>(), c.volumes.clone())
    };
    ctx.em.event(
        "scan_done",
        json!({
            "cancelled": cancelled,
            "resumable": !ctx.cat.read().unwrap().pending.is_empty(),
            "found": n,
            "bytes": b,
            "volumes": v,
            "seconds": t0.elapsed().as_secs_f64(),
            "errors": ctx.src.read_errors.load(std::sync::atomic::Ordering::Relaxed),
        }),
    );
}

/// If the same content was found more than once, marks all but the best copy.
fn dedupe(ctx: &Ctx) {
    let ids: Vec<(u64, u32)> = {
        let c = ctx.cat.read().unwrap();
        let mut sizes: HashMap<u64, u32> = HashMap::new();
        for e in c.entries.iter().filter(|e| e.size > 0 && e.state != State::Snapshot && e.state != State::Existing) {
            *sizes.entry(e.size).or_default() += 1;
        }
        // Read only files of equal size: the real cost is disk access.
        c.entries
            .iter()
            .filter(|e| e.size > 0 && e.state != State::Snapshot && e.state != State::Existing && sizes[&e.size] > 1)
            .map(|e| (e.first_off().unwrap_or(0), e.id))
            .collect()
    };
    let mut ids = ids;
    ids.sort_unstable();
    ctx.set_phase("phase.dedupe", ids.len() as u64);
    let mut best: HashMap<(u64, u64), (u32, u8)> = HashMap::new();
    let mut dups: Vec<u32> = Vec::new();
    for (i, (_, id)) in ids.iter().enumerate() {
        if i % 64 == 0 {
            ctx.advance(i as u64);
            if !ctx.alive() {
                return;
            }
        }
        let e = ctx.cat.read().unwrap().entries[*id as usize].clone();
        let head = reader::head(&ctx.src, &e, 65536);
        if head.iter().all(|c| *c == 0) {
            continue;
        }
        let key = (e.size, xxhash_rust::xxh64::xxh64(&head, 0));
        match best.get(&key) {
            Some((other, ch)) if *ch >= e.chance => {
                let _ = other;
                dups.push(*id);
            }
            Some((other, _)) => {
                dups.push(*other);
                best.insert(key, (*id, e.chance));
            }
            None => {
                best.insert(key, (*id, e.chance));
            }
        }
    }
    let mut c = ctx.cat.write().unwrap();
    for id in dups {
        c.entries[id as usize].dup = true;
    }
    c.version += 1;
}

/// Content verification: checks the start of a file on disk and brings the chance
/// of records zeroed by TRIM or overwritten down to reality.
fn verify(ctx: &Ctx) {
    let mut order: Vec<(u64, u32)> = {
        let c = ctx.cat.read().unwrap();
        c.entries
            .iter()
            .filter(|e| !e.verified && matches!(e.state, State::Deleted | State::Orphan))
            .map(|e| (e.first_off().unwrap_or(0), e.id))
            .collect()
    };
    order.sort_unstable();
    ctx.set_phase("phase.verify", order.len() as u64);
    for (i, (_, id)) in order.iter().enumerate() {
        if i % 64 == 0 {
            ctx.advance(i as u64);
            if !ctx.alive() {
                return;
            }
        }
        let e = {
            let c = ctx.cat.read().unwrap();
            c.entries[*id as usize].clone()
        };
        let head = reader::head(&ctx.src, &e, 4096);
        let zero_head = !head.is_empty() && head.iter().all(|c| *c == 0);
        let sniffed = sigs::sniff(&head);
        let mut chance = e.chance;
        let mut note = e.note.clone();
        let mut new_ext: Option<String> = None;
        if e.ext.is_empty() || e.name.starts_with("inode_") {
            if let Some(s) = sniffed {
                new_ext = Some(s.to_string());
            }
        } else if !zero_head {
            if let Some(fam) = sigs::family(&e.ext) {
                match sniffed {
                    Some(s) if sigs::family_of_sniff(s) == fam => chance = chance.clamp(90, 99),
                    _ => {
                        chance = chance.min(12);
                        note = Some(format!("note.sig_mismatch|{}", e.ext));
                    }
                }
            }
        }
        // TRIM erases a file piece by piece: the head can be zero with an intact body,
        // and vice versa. Large files are sampled along their length.
        let n = (e.size / (256 << 10)).clamp(0, 16);
        if n >= 4 {
            let mut zeros = u64::from(zero_head);
            for k in 1..=n {
                let at = (e.size - 4096) / n * k;
                let b = reader::range(&ctx.src, &e, at, 4096);
                if !b.is_empty() && b.iter().all(|c| *c == 0) {
                    zeros += 1;
                }
            }
            if zeros == n + 1 {
                chance = chance.min(2);
                note = Some("note.zeroed".into());
            } else if zeros * 2 >= n {
                chance = chance.min(15);
                note = Some("note.mostly_zero".into());
            } else if zeros * 4 >= n {
                // A single zero block is natural (disk images, padding); a quarter of them is not.
                chance = chance.min(40);
                note = Some("note.part_zero".into());
            }
        } else if zero_head {
            chance = chance.min(2);
            note = Some("note.zeroed".into());
        }
        let mut c = ctx.cat.write().unwrap();
        let t = &mut c.entries[*id as usize];
        t.chance = chance;
        t.note = note;
        if let Some(x) = new_ext {
            t.name = format!("{}.{x}", t.name);
            t.ext = x;
            t.kind = Kind::from_ext(&t.ext);
        }
        c.version += 1;
    }
}
