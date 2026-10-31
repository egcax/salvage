//! btrfs: thanks to copy-on-write, old tree nodes stay on disk. Nodes that are
//! unreachable from live trees are scanned, and the inode and extent records of
//! deleted files are collected from them. Files kept in snapshots but missing
//! from the live subvolume are found the same way.

use crate::codec;
use crate::ctx::*;
use crate::model::*;
use crate::source::*;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

const MAGIC: &[u8] = b"_BHRfS_M";
const HDR: usize = 101;
const TY_INODE: u8 = 1;
const TY_REF: u8 = 12;
const TY_EXTENT: u8 = 108;
const TY_ROOT: u8 = 132;
const TY_ROOTREF: u8 = 156;
const TY_EXTENT_ITEM: u8 = 168;
const TY_METADATA_ITEM: u8 = 169;
const TY_CHUNK: u8 = 228;
const TY_CSUM: u8 = 128;
const CSUM_TREE: u64 = 7;
const CSUM_OBJ: u64 = u64::MAX - 9;
const FS_TREE: u64 = 5;
const FIRST_FREE: u64 = 256;

pub fn detect(sb: &[u8]) -> bool {
    sb.len() > 0x48 && &sb[0x40..0x48] == MAGIC
}

#[derive(Clone)]
struct Chunk {
    len: u64,
    stripe_len: u64,
    ty: u64,
    stripes: Vec<(u64, u64)>,
}

struct Bt<'a> {
    v: &'a Vol,
    fsid: [u8; 16],
    nodesize: usize,
    sector: usize,
    csum: u16,
    devid: u64,
    chunks: BTreeMap<u64, Chunk>,
    cache: HashMap<u64, Option<Arc<Vec<u8>>>>,
}

#[derive(Clone, Copy, Debug)]
struct Key {
    obj: u64,
    ty: u8,
    off: u64,
}

fn key_at(b: &[u8], o: usize) -> Key {
    Key { obj: u64le(b, o), ty: u8_at(b, o + 8), off: u64le(b, o + 9) }
}

fn leaf_items(n: &[u8]) -> impl Iterator<Item = (Key, &[u8])> {
    let count = u32le(n, 96) as usize;
    (0..count).filter_map(move |i| {
        let e = HDR + i * 25;
        let k = key_at(n, e);
        let off = u32le(n, e + 17) as usize;
        let size = u32le(n, e + 21) as usize;
        n.get(HDR + off..HDR + off + size).map(|d| (k, d))
    })
}

impl<'a> Bt<'a> {
    fn map(&self, logical: u64) -> Option<u64> {
        let (start, c) = self.chunks.range(..=logical).next_back()?;
        let off = logical - start;
        if off >= c.len {
            return None;
        }
        const RAID0: u64 = 1 << 3;
        if c.ty & RAID0 != 0 && c.stripes.len() > 1 {
            let n = c.stripes.len() as u64;
            let nr = off / c.stripe_len;
            let (dev, phys) = c.stripes[(nr % n) as usize];
            if dev != self.devid {
                return None;
            }
            return Some(phys + (nr / n) * c.stripe_len + off % c.stripe_len);
        }
        let s = c.stripes.iter().find(|s| s.0 == self.devid).or(c.stripes.first())?;
        Some(s.1 + off)
    }

    fn valid_node(&self, b: &[u8], logical: u64) -> bool {
        if b[32..48] != self.fsid || u64le(b, 48) != logical {
            return false;
        }
        match self.csum {
            0 => {
                let c = crc32c::crc32c(&b[32..]);
                u32le(b, 0) == c
            }
            1 => xxhash_rust::xxh64::xxh64(&b[32..], 0) == u64le(b, 0),
            2 => {
                use sha2::Digest;
                let h = sha2::Sha256::digest(&b[32..]);
                h[..] == b[0..32]
            }
            _ => true,
        }
    }

    fn node(&mut self, logical: u64) -> Option<Arc<Vec<u8>>> {
        if let Some(n) = self.cache.get(&logical) {
            return n.clone();
        }
        let n = self.map(logical).and_then(|p| {
            let b = self.v.read(p, self.nodesize);
            self.valid_node(&b, logical).then(|| Arc::new(b))
        });
        if self.cache.len() > 8192 {
            self.cache.clear();
        }
        self.cache.insert(logical, n.clone());
        n
    }

    /// Walks a tree; `visit` is called before each leaf.
    fn walk(&mut self, root: u64, visit: &mut dyn FnMut(u64, &[u8])) {
        let mut stack = vec![root];
        while let Some(l) = stack.pop() {
            let Some(n) = self.node(l) else { continue };
            if n[100] == 0 {
                visit(l, &n);
            } else {
                let cnt = u32le(&n, 96) as usize;
                for i in (0..cnt).rev() {
                    let p = u64le(&n, HDR + i * 33 + 17);
                    stack.push(p);
                }
            }
        }
    }

    /// Walks the internal nodes and collects all block addresses of the tree.
    fn reach(&mut self, root: u64, set: &mut HashSet<u64>) {
        let mut stack = vec![root];
        while let Some(l) = stack.pop() {
            if !set.insert(l) {
                continue;
            }
            let Some(n) = self.node(l) else { continue };
            if n[100] > 0 {
                let cnt = u32le(&n, 96) as usize;
                for i in 0..cnt {
                    stack.push(u64le(&n, HDR + i * 33 + 17));
                }
            }
        }
    }

    /// Finds the first item matching (obj,ty).
    fn find(&mut self, root: u64, obj: u64, ty: u8) -> Option<Vec<u8>> {
        let mut cur = root;
        let mut guard = 0;
        loop {
            guard += 1;
            if guard > 16 {
                return None;
            }
            let n = self.node(cur)?;
            let cnt = u32le(&n, 96) as usize;
            if n[100] == 0 {
                for (k, d) in leaf_items(&n) {
                    if k.obj == obj && k.ty == ty {
                        return Some(d.to_vec());
                    }
                    if k.obj > obj || (k.obj == obj && k.ty > ty) {
                        return None;
                    }
                }
                return None;
            }
            let mut pick = 0;
            for i in 0..cnt {
                let k = key_at(&n, HDR + i * 33);
                if (k.obj, k.ty) < (obj, ty) || (k.obj, k.ty, k.off) == (obj, ty, 0) {
                    pick = i;
                } else {
                    break;
                }
            }
            cur = u64le(&n, HDR + pick * 33 + 17);
        }
    }
}

fn parse_chunk(d: &[u8]) -> Option<(Chunk, usize)> {
    let len = u64le(d, 0);
    let stripe_len = u64le(d, 16);
    let ty = u64le(d, 24);
    let num = u16le(d, 44) as usize;
    let total = 48 + num * 32;
    if num == 0 || d.len() < total {
        return None;
    }
    let stripes = (0..num).map(|i| (u64le(d, 48 + i * 32), u64le(d, 56 + i * 32))).collect();
    Some((Chunk { len, stripe_len, ty, stripes }, total))
}

#[derive(Clone, Default)]
struct Inode {
    gen: u64,
    size: u64,
    size_gen: u64,
    mode: u32,
    mtime: i64,
    ctime: i64,
    birth: u64,
}

fn parse_inode(d: &[u8]) -> Inode {
    Inode {
        gen: 0,
        size_gen: 0,
        size: u64le(d, 16),
        mode: u32le(d, 52),
        mtime: u64le(d, 136) as i64,
        ctime: u64le(d, 124) as i64,
        birth: u64le(d, 0),
    }
}

#[derive(Clone)]
struct Ext {
    gen: u64,
    /// Transaction that wrote the extent; an older csum belongs to another file.
    written: u64,
    ram: u64,
    comp: u8,
    ty: u8,
    bytenr: u64,
    disk_len: u64,
    off: u64,
    num: u64,
    inline: Option<Vec<u8>>,
}

fn parse_extent(d: &[u8]) -> Option<Ext> {
    if d.len() < 21 {
        return None;
    }
    let ty = d[20];
    let mut e = Ext {
        gen: 0,
        written: u64le(d, 0),
        ram: u64le(d, 8),
        comp: d[16],
        ty,
        bytenr: 0,
        disk_len: 0,
        off: 0,
        num: u64le(d, 8),
        inline: None,
    };
    if d[17] != 0 || u16le(d, 18) != 0 {
        return None;
    }
    if ty == 0 {
        e.inline = Some(d[21..].to_vec());
    } else {
        e.bytenr = u64le(d, 21);
        e.disk_len = u64le(d, 29);
        e.off = u64le(d, 37);
        e.num = u64le(d, 45);
    }
    Some(e)
}

#[derive(Default)]
struct Stale {
    inodes: HashMap<(u64, u64), Inode>,
    refs: HashMap<(u64, u64), (u64, u64, String)>, // (gen, parent, name)
    exts: HashMap<(u64, u64), BTreeMap<u64, Ext>>,
    snap: HashSet<(u64, u64)>,
}

pub fn scan(v: &Vol, ctx: &Ctx, vol: u16, root: u32) -> Result<FsResult, String> {
    let sb = v.read(0x10000, 4096);
    if !detect(&sb) {
        return Err("btrfs superblock not found".into());
    }
    let mut fsid = [0u8; 16];
    // With the metadata_uuid feature, nodes use that value.
    let incompat = u64le(&sb, 0xbc);
    fsid.copy_from_slice(if incompat & (1 << 10) != 0 { &sb[0x23b..0x24b] } else { &sb[0x20..0x30] });
    let nodesize = u32le(&sb, 0x94) as usize;
    let sector = u32le(&sb, 0x90) as usize;
    let csum = u16le(&sb, 0xc4);
    let label = String::from_utf8_lossy(&sb[0x12b..0x22b]).trim_end_matches('\0').to_string();
    if !(4096..=65536).contains(&nodesize) || sector == 0 {
        return Err("invalid btrfs node size".into());
    }
    let devid = u64le(&sb, 0xc9);
    let mut bt = Bt { v, fsid, nodesize, sector, csum, devid, chunks: BTreeMap::new(), cache: HashMap::new() };

    // sys_chunk_array
    let arr_len = u32le(&sb, 0xa0) as usize;
    let arr = &sb[0x32b..(0x32b + arr_len).min(sb.len())];
    let mut p = 0;
    while p + 17 < arr.len() {
        let k = key_at(arr, p);
        p += 17;
        if k.ty != TY_CHUNK {
            break;
        }
        match parse_chunk(&arr[p..]) {
            Some((c, n)) => {
                bt.chunks.insert(k.off, c);
                p += n;
            }
            None => break,
        }
    }
    // Chunk tree
    let chunk_root = u64le(&sb, 0x58);
    let mut list: Vec<(u64, Chunk)> = Vec::new();
    bt.walk(chunk_root, &mut |_, n| {
        for (k, d) in leaf_items(n) {
            if k.ty == TY_CHUNK {
                if let Some((c, _)) = parse_chunk(d) {
                    list.push((k.off, c));
                }
            }
        }
    });
    if list.is_empty() {
        return Err("cannot read btrfs chunk tree".into());
    }
    bt.cache.clear();
    for (o, c) in list {
        bt.chunks.insert(o, c);
    }

    // Root tree: subvolumes, names, parent links.
    ctx.set_phase("phase.subvols", 0);
    let root_tree = u64le(&sb, 0x50);
    let mut subs: HashMap<u64, u64> = HashMap::new(); // id -> root node
    let mut uuids: HashMap<[u8; 16], u64> = HashMap::new();
    let mut parents: HashMap<u64, [u8; 16]> = HashMap::new();
    let mut names: HashMap<u64, (u64, String)> = HashMap::new();
    let mut extent_root = 0;
    let mut csum_root = 0;
    bt.walk(root_tree, &mut |_, n| {
        for (k, d) in leaf_items(n) {
            if k.ty == TY_ROOT {
                if k.obj == 2 {
                    extent_root = u64le(d, 176);
                }
                if k.obj == CSUM_TREE {
                    csum_root = u64le(d, 176);
                }
                if k.obj == FS_TREE || (FIRST_FREE..u64::MAX - 255).contains(&k.obj) {
                    subs.insert(k.obj, u64le(d, 176));
                    if d.len() >= 279 {
                        let mut u = [0u8; 16];
                        u.copy_from_slice(&d[247..263]);
                        let mut pu = [0u8; 16];
                        pu.copy_from_slice(&d[263..279]);
                        uuids.insert(u, k.obj);
                        if pu != [0u8; 16] {
                            parents.insert(k.obj, pu);
                        }
                    }
                }
            } else if k.ty == TY_ROOTREF {
                let nl = u16le(d, 16) as usize;
                if let Some(nb) = d.get(18..18 + nl) {
                    names.insert(k.off, (k.obj, String::from_utf8_lossy(nb).into_owned()));
                }
            }
        }
    });
    let parent_of = |id: u64| parents.get(&id).and_then(|u| uuids.get(u)).copied();
    let is_snap: HashSet<u64> = subs.keys().copied().filter(|i| parents.contains_key(i)).collect();
    let snap_count = is_snap.len();

    // Used space: extent tree.
    ctx.set_phase("phase.usage", 0);
    let mut logical: Vec<(u64, u64)> = Vec::new();
    if extent_root != 0 {
        let ns = nodesize as u64;
        let mut n_seen = 0u64;
        bt.walk(extent_root, &mut |_, n| {
            for (k, _) in leaf_items(n) {
                if k.ty == TY_EXTENT_ITEM {
                    logical.push((k.obj, k.obj + k.off));
                } else if k.ty == TY_METADATA_ITEM {
                    logical.push((k.obj, k.obj + ns));
                }
            }
            n_seen += 1;
            if n_seen % 1024 == 0 {
                ctx.advance(n_seen);
            }
        });
    }
    logical.sort_unstable();
    let mut used = Intervals::default();
    for (s, e) in &logical {
        used.push(*s, *e);
    }
    used.normalize();
    drop(logical);
    let mut alloc = Intervals::default();
    alloc.push(v.abs(0), v.abs(1 << 20));
    for (start, c) in &bt.chunks {
        let items = used.items();
        let first = items.partition_point(|x| x.1 <= *start);
        for &(s, e) in &items[first..] {
            if s >= start + c.len {
                break;
            }
            let a = s.max(*start) - start;
            let b = e.min(start + c.len) - start;
            for st in &c.stripes {
                if st.0 == devid {
                    alloc.push(v.abs(st.1 + a), v.abs(st.1 + b));
                }
            }
        }
    }
    alloc.normalize();

    // Live blocks: those reachable from subvolumes that are NOT snapshots.
    ctx.set_phase("phase.live", subs.len() as u64);
    let mut live: HashSet<u64> = HashSet::new();
    let mut ids: Vec<(u64, u64)> = subs.iter().map(|(a, b)| (*a, *b)).collect();
    ids.sort();
    for (i, (id, node)) in ids.iter().enumerate() {
        if !is_snap.contains(id) {
            bt.reach(*node, &mut live);
        }
        ctx.advance(i as u64);
        if !ctx.alive() {
            break;
        }
    }
    let mut snap_blocks: HashSet<u64> = HashSet::new();
    for (id, node) in &ids {
        if is_snap.contains(id) {
            bt.reach(*node, &mut snap_blocks);
        }
    }
    let live = Arc::new(live);
    // Live csum leaves belong to new files: a reused address could make an
    // old file look intact, so only old leaves count.
    let mut live_csum: HashSet<u64> = HashSet::new();
    if csum_root != 0 {
        bt.reach(csum_root, &mut live_csum);
    }
    let mut csum_leaves: Vec<(u64, u64)> = Vec::new();

    // Scan the metadata chunks linearly.
    let meta: Vec<(u64, u64)> = bt.chunks.iter().filter(|(_, c)| c.ty & 4 != 0).map(|(s, c)| (*s, c.len)).collect();
    let total: u64 = meta.iter().map(|(_, l)| *l).sum();
    ctx.set_phase("phase.stale", total);
    let mut st = Stale::default();
    let mut done = 0u64;
    let mut alive_cache: HashMap<(u64, u64), bool> = HashMap::new();
    let mut zero_nodes = 0u64;
    let mut seen_nodes = 0u64;
    let mut stale_nodes = 0u64;
    let buf_size = nodesize * 64;
    let mut buf = vec![0u8; buf_size];
    for (start, len) in meta {
        let Some(phys) = bt.map(start) else { continue };
        ctx.mark(v.abs(phys), v.abs(phys + len), CELL_META);
        let mut o = 0u64;
        while o < len {
            if !ctx.alive() {
                break;
            }
            let n = ((len - o) as usize).min(buf_size);
            let Some(p2) = bt.map(start + o) else {
                o += n as u64;
                continue;
            };
            v.read_into(p2, &mut buf[..n]);
            v.src.drop_cache(v.abs(p2), n as u64);
            for (i, node) in buf[..n].chunks_exact(nodesize).enumerate() {
                let l = start + o + (i * nodesize) as u64;
                seen_nodes += 1;
                if live.contains(&l) {
                    continue;
                }
                if node[..64].iter().all(|c| *c == 0) {
                    zero_nodes += 1;
                    continue;
                }
                if node[100] != 0 || !bt.valid_node(node, l) {
                    continue;
                }
                let owner = u64le(node, 88);
                if owner == CSUM_TREE && !live_csum.contains(&l) {
                    csum_leaves.push((u64le(node, 80), l));
                    continue;
                }
                if owner != FS_TREE && !(FIRST_FREE..u64::MAX - 255).contains(&owner) {
                    continue;
                }
                stale_nodes += 1;
                let from_snap = snap_blocks.contains(&l);
                let cmp_root = if is_snap.contains(&owner) { parent_of(owner).unwrap_or(owner) } else { owner };
                ingest(&mut bt, node, cmp_root, from_snap, &subs, &mut alive_cache, &mut st);
            }
            o += n as u64;
            done += n as u64;
            ctx.advance(done);
        }
    }
    ctx.log(format!(
        "btrfs: {seen_nodes} nodes, {zero_nodes} zeroed, {stale_nodes} stale leaves, {} stale csum leaves, {snap_count} snapshots",
        csum_leaves.len()
    ));

    // Pick sample sectors from deleted extents, find their values in old csum leaves.
    let plans = sample_plan(&st, sector as u64);
    let mut wanted: BTreeMap<u64, Option<(u64, Vec<u8>)>> = BTreeMap::new();
    for (l, _) in plans.values().flatten() {
        wanted.insert(*l, None);
    }
    let csize = match csum {
        0 => 4,
        1 => 8,
        _ => 32,
    };
    ctx.set_phase("phase.csum", csum_leaves.len() as u64);
    csum_leaves.sort_unstable();
    for (i, (gen, l)) in csum_leaves.iter().enumerate() {
        if i % 256 == 0 {
            ctx.advance(i as u64);
            if !ctx.alive() {
                break;
            }
        }
        let Some(node) = bt.node(*l) else { continue };
        for (k, d) in leaf_items(&node) {
            if k.obj != CSUM_OBJ || k.ty != TY_CSUM {
                continue;
            }
            let end = k.off + (d.len() / csize) as u64 * sector as u64;
            for (lg, slot) in wanted.range_mut(k.off..end) {
                let idx = ((lg - k.off) / sector as u64) as usize * csize;
                if let Some(c) = d.get(idx..idx + csize) {
                    *slot = Some((*gen, c.to_vec()));
                }
            }
        }
    }
    ctx.set_phase("phase.sectors", wanted.len() as u64);
    let mut sector_ok: HashMap<u64, (u64, bool)> = HashMap::new();
    let mut buf = vec![0u8; sector];
    for (i, (l, slot)) in wanted.iter().enumerate() {
        if i % 1024 == 0 {
            ctx.advance(i as u64);
            if !ctx.alive() {
                break;
            }
        }
        let (Some((gen, want)), Some(phys)) = (slot, bt.map(*l)) else { continue };
        v.read_into(phys, &mut buf);
        let same = match csum {
            0 => crc32c::crc32c(&buf).to_le_bytes()[..] == want[..4],
            1 => xxhash_rust::xxh64::xxh64(&buf, 0).to_le_bytes()[..] == want[..8],
            _ => {
                use sha2::Digest;
                sha2::Sha256::digest(&buf)[..] == want[..]
            }
        };
        sector_ok.insert(*l, (*gen, same));
    }

    // Turn the results into file entries.
    ctx.set_phase("phase.resolve", st.inodes.len() as u64);
    let lost = ctx.cat.write().unwrap().child_dir(root, "$Orphans", true);
    let mut dirmap: HashMap<(u64, u64), u32> = HashMap::new();
    let mut keys: Vec<(u64, u64)> = st.exts.keys().copied().collect();
    for k in st.inodes.keys() {
        if !st.exts.contains_key(k) {
            keys.push(*k);
        }
    }
    for (i, key) in keys.iter().enumerate() {
        if i % 512 == 0 {
            ctx.advance(i as u64);
            if !ctx.alive() {
                break;
            }
        }
        let inode = st.inodes.get(key).cloned();
        if let Some(n) = &inode {
            if n.mode & 0o170000 != 0o100000 {
                continue;
            }
        }
        let Some(ex) = st.exts.get(key) else { continue };
        let size = match &inode {
            Some(n) if n.size > 0 => n.size,
            _ => ex.iter().map(|(o, e)| o + e.num).max().unwrap_or(0),
        };
        if size == 0 {
            continue;
        }
        let (pieces, disk, overlap) = build_pieces(&bt, ex, size, &used);
        if pieces.is_empty() {
            continue;
        }
        let (rootid, ino) = *key;
        let dir = {
            let mut cx = Resolve { bt: &mut bt, st: &st, ctx, subs: &subs, names: &names, map: &mut dirmap, lost, root };
            match st.refs.get(key) {
                Some((_, parent, _)) => cx.dir(rootid, *parent, 0),
                None => cx.vol_root(rootid),
            }
        };
        let name = st.refs.get(key).map(|r| r.2.clone()).unwrap_or_else(|| format!("inode_{ino}"));
        let snapshot = st.snap.contains(key);
        let mut e = Entry::new(
            vol,
            dir,
            name,
            size,
            if snapshot { State::Snapshot } else { State::Deleted },
            if snapshot { "btrfs snapshot" } else { "btrfs stale tree" },
        );
        if let Some(n) = &inode {
            e.mtime = n.mtime;
            e.ctime = n.ctime;
        }
        e.chance = if snapshot {
            99
        } else if disk == 0 {
            96
        } else {
            let f = overlap as f64 / disk as f64;
            if f == 0.0 {
                88
            } else if f < 0.2 {
                40
            } else {
                4
            }
        };
        let holes: u64 = pieces.iter().filter(|p| matches!(p, Piece::Zero { .. })).map(|p| p.out_len()).sum();
        if holes * 100 > size {
            e.chance = e.chance.min(40);
            e.note = Some("note.extents_lost".into());
        }
        // Measured integrity takes priority over estimates.
        if !snapshot {
            let limit = inode.as_ref().map(|n| n.gen + 2).unwrap_or(u64::MAX);
            let (mut good, mut bad) = (0u32, 0u32);
            for (l, written) in plans.get(key).map(Vec::as_slice).unwrap_or(&[]) {
                match sector_ok.get(l) {
                    Some((g, true)) if *g <= limit && g >= written => good += 1,
                    Some((g, false)) if *g <= limit && g >= written => bad += 1,
                    _ => {}
                }
            }
            if good + bad >= 1 {
                let ratio = good as f64 / (good + bad) as f64;
                e.verified = true;
                if bad == 0 {
                    e.chance = 97;
                    e.note = Some(format!("note.csum_ok|{good}"));
                } else if good == 0 {
                    e.chance = 2;
                    e.note = Some("note.csum_bad".into());
                } else {
                    e.chance = (ratio * 78.0) as u8;
                    e.note = Some(format!("note.csum_part|{:.0}", ratio * 100.0));
                }
            }
        }
        e.pieces = pieces;
        ctx.add(e);
    }

    let note = (snap_count > 0).then(|| format!("note.snapshots|{snap_count}"));
    Ok(FsResult { fstype: "btrfs".into(), label, block_size: sector as u32, alloc, note })
}

fn ingest(
    bt: &mut Bt,
    node: &[u8],
    root: u64,
    from_snap: bool,
    subs: &HashMap<u64, u64>,
    alive: &mut HashMap<(u64, u64), bool>,
    st: &mut Stale,
) {
    let gen = u64le(node, 80);
    for (k, d) in leaf_items(node) {
        if !matches!(k.ty, TY_INODE | TY_REF | TY_EXTENT) || k.obj < FIRST_FREE {
            continue;
        }
        let key = (root, k.obj);
        let is_alive = *alive.entry(key).or_insert_with(|| match subs.get(&root) {
            Some(r) => bt.find(*r, k.obj, TY_INODE).is_some(),
            None => false,
        });
        if is_alive {
            continue;
        }
        if from_snap {
            st.snap.insert(key);
        }
        match k.ty {
            TY_INODE => {
                // Deletion zeroes the size and writes a new generation: the real size is in
                // the newest record with a non-zero size.
                let n = parse_inode(d);
                let cur = st.inodes.entry(key).or_default();
                if n.size > 0 && gen >= cur.size_gen {
                    cur.size = n.size;
                    cur.size_gen = gen;
                }
                if gen >= cur.gen {
                    cur.gen = gen;
                    cur.mode = n.mode;
                    cur.mtime = n.mtime;
                    cur.ctime = n.ctime;
                    cur.birth = n.birth;
                }
            }
            TY_REF => {
                let nl = u16le(d, 8) as usize;
                if let Some(nb) = d.get(10..10 + nl) {
                    let e = st.refs.entry(key).or_insert((0, 0, String::new()));
                    if gen >= e.0 {
                        *e = (gen, k.off, String::from_utf8_lossy(nb).into_owned());
                    }
                }
            }
            TY_EXTENT => {
                if let Some(mut e) = parse_extent(d) {
                    e.gen = gen;
                    let m = st.exts.entry(key).or_default();
                    match m.get(&k.off) {
                        Some(c) if c.gen >= gen => {}
                        _ => {
                            m.insert(k.off, e);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Sectors to verify per file. The first sector is always included: TRIM usually
/// starts there. Total reads scale with size, not with the number of files.
fn sample_plan(st: &Stale, sector: u64) -> HashMap<(u64, u64), Vec<(u64, u64)>> {
    let mut out = HashMap::new();
    for (key, ex) in &st.exts {
        if st.snap.contains(key) {
            continue;
        }
        let mut sectors: Vec<(u64, u64, u64)> = Vec::new();
        let mut total = 0u64;
        for e in ex.values() {
            if e.inline.is_some() || e.bytenr == 0 || e.ty == 2 {
                continue;
            }
            // In a compressed extent, csum covers the compressed data on disk.
            let (s, n) = if e.comp != 0 { (e.bytenr, e.disk_len) } else { (e.bytenr + e.off, e.num) };
            let a = s.div_ceil(sector) * sector;
            if s + n > a {
                sectors.push((a, (s + n - a) / sector, e.written));
                total += (s + n - a) / sector;
            }
        }
        if total == 0 {
            continue;
        }
        let want = (2 + total / 512).clamp(2, 32).min(total);
        let mut picks = Vec::with_capacity(want as usize);
        for k in 0..want {
            let mut idx = if want == 1 { 0 } else { k * (total - 1) / (want - 1) };
            for (start, cnt, written) in &sectors {
                if idx < *cnt {
                    picks.push((start + idx * sector, *written));
                    break;
                }
                idx -= cnt;
            }
        }
        out.insert(*key, picks);
    }
    out
}

fn build_pieces(bt: &Bt, ex: &BTreeMap<u64, Ext>, size: u64, used: &Intervals) -> (Vec<Piece>, u64, u64) {
    let mut out: Vec<Piece> = Vec::new();
    let mut pos = 0u64;
    let (mut disk, mut overlap) = (0u64, 0u64);
    // Older generation records can cover the range of newer ones; the newest gen wins.
    let top = ex.values().map(|e| e.gen).max().unwrap_or(0);
    let _ = top;
    for (off, e) in ex {
        if *off >= size {
            break;
        }
        if *off < pos {
            continue;
        }
        if *off > pos {
            out.push(Piece::Zero { len: off - pos });
            pos = *off;
        }
        let want = (size - pos).min(e.num);
        if let Some(data) = &e.inline {
            let d = if e.comp != 0 {
                codec::decompress(e.comp, data, e.ram as usize, bt.sector).unwrap_or_default()
            } else {
                data.clone()
            };
            let take = (d.len() as u64).min(want) as usize;
            out.push(Piece::Inline { data: d[..take].to_vec() });
            pos += take as u64;
            continue;
        }
        if e.bytenr == 0 || e.ty == 2 {
            out.push(Piece::Zero { len: want });
            pos += want;
            continue;
        }
        let Some(phys) = bt.map(e.bytenr) else {
            out.push(Piece::Zero { len: want });
            pos += want;
            continue;
        };
        disk += e.disk_len;
        overlap += used.overlap(e.bytenr, e.bytenr + e.disk_len);
        let abs = bt.v.abs(phys);
        if e.comp != 0 {
            out.push(Piece::Comp { off: abs, clen: e.disk_len, codec: e.comp, ulen: e.ram, skip: e.off, len: want });
        } else {
            let a = abs + e.off;
            match out.last_mut() {
                Some(Piece::Data { off: o, len }) if *o + *len == a => *len += want,
                _ => out.push(Piece::Data { off: a, len: want }),
            }
        }
        pos += want;
    }
    if pos < size && !out.is_empty() {
        out.push(Piece::Zero { len: size - pos });
    }
    (out, disk, overlap)
}

struct Resolve<'a, 'b> {
    bt: &'a mut Bt<'b>,
    st: &'a Stale,
    ctx: &'a Ctx,
    subs: &'a HashMap<u64, u64>,
    names: &'a HashMap<u64, (u64, String)>,
    map: &'a mut HashMap<(u64, u64), u32>,
    lost: u32,
    root: u32,
}

impl Resolve<'_, '_> {
    fn vol_root(&mut self, id: u64) -> u32 {
        if let Some(d) = self.map.get(&(id, 0)) {
            return *d;
        }
        let name = self.names.get(&id).map(|n| n.1.clone()).unwrap_or_else(|| {
            if id == FS_TREE {
                "root".to_string()
            } else {
                format!("subvol_{id}")
            }
        });
        let d = self.ctx.cat.write().unwrap().child_dir(self.root, &name, false);
        self.map.insert((id, 0), d);
        d
    }

    fn dir(&mut self, root: u64, ino: u64, depth: u32) -> u32 {
        if ino == FIRST_FREE {
            return self.vol_root(root);
        }
        if let Some(d) = self.map.get(&(root, ino)) {
            return *d;
        }
        if depth > 256 {
            return self.lost;
        }
        // Look in the live tree first, then in old records.
        let live_ref = self
            .subs
            .get(&root)
            .copied()
            .and_then(|r| ref_in_tree(self.bt, r, ino));
        let info = live_ref
            .map(|(p, n)| (p, n, false))
            .or_else(|| self.st.refs.get(&(root, ino)).map(|r| (r.1, r.2.clone(), true)));
        let id = match info {
            Some((parent, name, del)) if parent != ino => {
                let up = self.dir(root, parent, depth + 1);
                self.ctx.cat.write().unwrap().child_dir(up, &name, del)
            }
            _ => self.lost,
        };
        self.map.insert((root, ino), id);
        id
    }
}

fn ref_in_tree(bt: &mut Bt, root: u64, ino: u64) -> Option<(u64, String)> {
    // INODE_REF key (ino, 12, parent): goes straight to the leaf and scans it.
    let mut cur = root;
    for _ in 0..16 {
        let n = bt.node(cur)?;
        let cnt = u32le(&n, 96) as usize;
        if n[100] == 0 {
            for (k, d) in leaf_items(&n) {
                if k.obj == ino && k.ty == TY_REF {
                    let nl = u16le(d, 8) as usize;
                    return d.get(10..10 + nl).map(|nb| (k.off, String::from_utf8_lossy(nb).into_owned()));
                }
            }
            return None;
        }
        let mut pick = 0;
        for i in 0..cnt {
            let k = key_at(&n, HDR + i * 33);
            if (k.obj, k.ty) <= (ino, TY_REF) {
                pick = i;
            } else {
                break;
            }
        }
        cur = u64le(&n, HDR + pick * 33 + 17);
    }
    None
}
