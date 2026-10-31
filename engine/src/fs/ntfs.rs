//! NTFS: $MFT records, deleted files, extension records, compression and the
//! Recycle Bin. In deep mode, volumes with a damaged MFT are swept for FILE signatures.

use crate::ctx::*;
use crate::model::*;
use crate::source::*;
use std::collections::HashMap;

const IN_USE: u16 = 1;
const IS_DIR: u16 = 2;
const ROOT_REC: u64 = 5;

pub fn detect(b: &[u8]) -> bool {
    b.len() >= 512 && &b[3..11] == b"NTFS    " && u16le(b, 510) == 0xAA55
}

fn filetime(t: u64) -> i64 {
    if t == 0 {
        return 0;
    }
    (t / 10_000_000) as i64 - 11_644_473_600
}

#[derive(Default, Clone)]
struct DataAttr {
    runs: Vec<(u64, Option<u64>, u64)>, // (vcn, lcn, length)
    size: u64,
    init: u64,
    resident: Option<Vec<u8>>,
    compressed: bool,
    comp_unit: u8,
    encrypted: bool,
    start_vcn: u64,
}

#[derive(Default, Clone)]
struct Rec {
    no: u64,
    seq: u16,
    flags: u16,
    base: u64,
    name: String,
    ns: u8,
    parent: u64,
    parent_seq: u16,
    mtime: i64,
    ctime: i64,
    atime: i64,
    data: Option<DataAttr>,
    has_attr_list: bool,
    index: Vec<(u64, Option<u64>, u64)>,
}

fn parse_runlist(b: &[u8], mut vcn: u64) -> Vec<(u64, Option<u64>, u64)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut lcn: i64 = 0;
    while i < b.len() {
        let hdr = b[i];
        if hdr == 0 {
            break;
        }
        let ln = (hdr & 0xF) as usize;
        let of = (hdr >> 4) as usize;
        i += 1;
        if ln == 0 || ln > 8 || of > 8 || i + ln + of > b.len() {
            break;
        }
        let mut len = 0u64;
        for k in 0..ln {
            len |= (b[i + k] as u64) << (8 * k);
        }
        i += ln;
        if of == 0 {
            out.push((vcn, None, len));
        } else {
            let mut d = 0i64;
            for k in 0..of {
                d |= (b[i + k] as i64) << (8 * k);
            }
            if b[i + of - 1] & 0x80 != 0 && of < 8 {
                d |= -1i64 << (8 * of);
            }
            lcn = lcn.wrapping_add(d);
            if lcn < 0 {
                break;
            }
            out.push((vcn, Some(lcn as u64), len));
        }
        i += of;
        vcn += len;
    }
    out
}

/// Update sequence fixup; returns false for torn records.
fn fixup(r: &mut [u8], sector: usize) -> bool {
    let off = u16le(r, 4) as usize;
    let cnt = u16le(r, 6) as usize;
    if cnt < 2 || off + cnt * 2 > r.len() {
        return false;
    }
    let usn = [r[off], r[off + 1]];
    for i in 1..cnt {
        let end = i * sector;
        if end > r.len() {
            break;
        }
        if r[end - 2] != usn[0] || r[end - 1] != usn[1] {
            return false;
        }
        r[end - 2] = r[off + i * 2];
        r[end - 1] = r[off + i * 2 + 1];
    }
    true
}

fn parse_record(rec: &mut [u8], no: u64, sector: usize) -> Option<Rec> {
    if &rec[0..4] != b"FILE" || !fixup(rec, sector) {
        return None;
    }
    let mut r = Rec {
        no,
        seq: u16le(rec, 16),
        flags: u16le(rec, 22),
        base: u64le(rec, 32) & 0xFFFF_FFFF_FFFF,
        ..Default::default()
    };
    let mut p = u16le(rec, 20) as usize;
    let mut guard = 0;
    while p + 16 <= rec.len() && guard < 64 {
        guard += 1;
        let ty = u32le(rec, p);
        if ty == 0xFFFF_FFFF {
            break;
        }
        let alen = u32le(rec, p + 4) as usize;
        if alen < 16 || p + alen > rec.len() {
            break;
        }
        let a = &rec[p..p + alen];
        let nonres = a[8] != 0;
        let name_len = a[9] as usize;
        let flags = u16le(a, 12);
        match ty {
            0x10 if !nonres => {
                let vo = u16le(a, 20) as usize;
                if let Some(v) = a.get(vo..) {
                    r.ctime = filetime(u64le(v, 0));
                    r.mtime = filetime(u64le(v, 8));
                    r.atime = filetime(u64le(v, 24));
                }
            }
            0x20 => r.has_attr_list = true,
            0x30 if !nonres => {
                let vo = u16le(a, 20) as usize;
                if let Some(v) = a.get(vo..) {
                    let ns = u8_at(v, 65);
                    let n = u8_at(v, 64) as usize;
                    // Use the DOS short name only if there is no other name.
                    if r.name.is_empty() || (r.ns == 2 && ns != 2) {
                        if let Some(nb) = v.get(66..66 + n * 2) {
                            r.name = utf16le_string(nb);
                            r.ns = ns;
                            let pr = u64le(v, 0);
                            r.parent = pr & 0xFFFF_FFFF_FFFF;
                            r.parent_seq = (pr >> 48) as u16;
                        }
                    }
                }
            }
            0xA0 if nonres => {
                let ro = u16le(a, 32) as usize;
                if let Some(rl) = a.get(ro..) {
                    r.index = parse_runlist(rl, u64le(a, 16));
                }
            }
            0x80 if name_len == 0 => {
                let mut d = DataAttr {
                    compressed: flags & 1 != 0,
                    encrypted: flags & 0x4000 != 0,
                    ..Default::default()
                };
                if nonres {
                    d.start_vcn = u64le(a, 16);
                    d.size = u64le(a, 48);
                    d.init = u64le(a, 56);
                    d.comp_unit = a[34];
                    let ro = u16le(a, 32) as usize;
                    if let Some(rl) = a.get(ro..) {
                        d.runs = parse_runlist(rl, d.start_vcn);
                    }
                } else {
                    let vl = u32le(a, 16) as usize;
                    let vo = u16le(a, 20) as usize;
                    d.size = vl as u64;
                    d.init = vl as u64;
                    d.resident = a.get(vo..vo + vl).map(|s| s.to_vec());
                }
                match &mut r.data {
                    None => r.data = Some(d),
                    Some(cur) => {
                        cur.runs.extend(d.runs);
                        cur.size = cur.size.max(d.size);
                        cur.init = cur.init.max(d.init);
                    }
                }
            }
            _ => {}
        }
        p += alen;
    }
    Some(r)
}

fn pieces_of(d: &DataAttr, cluster: u64, v: &Vol) -> Vec<Piece> {
    if let Some(res) = &d.resident {
        return vec![Piece::Inline { data: res.clone() }];
    }
    let mut runs = d.runs.clone();
    runs.sort_by_key(|r| r.0);
    let mut out: Vec<Piece> = Vec::new();
    if d.compressed && d.comp_unit > 0 {
        // Walk unit by unit: a fully allocated unit is uncompressed; if it has a sparse
        // part, the leading clusters are LZNT1 data.
        let unit = 1u64 << d.comp_unit;
        let mut clusters: Vec<Option<u64>> = Vec::new();
        for (_, lcn, len) in &runs {
            for k in 0..*len {
                clusters.push(lcn.map(|l| l + k));
            }
            if clusters.len() as u64 > d.size / cluster + unit * 2 {
                break;
            }
        }
        let mut left = d.size;
        for chunk in clusters.chunks(unit as usize) {
            if left == 0 {
                break;
            }
            let ulen = (unit * cluster).min(left);
            let used: Vec<u64> = chunk.iter().filter_map(|c| *c).collect();
            if used.is_empty() {
                out.push(Piece::Zero { len: ulen });
            } else if used.len() as u64 == unit {
                for c in used {
                    let take = cluster.min(ulen);
                    let off = v.abs(c * cluster);
                    match out.last_mut() {
                        Some(Piece::Data { off: o, len }) if *o + *len == off => *len += take,
                        _ => out.push(Piece::Data { off, len: take }),
                    }
                }
                if ulen < unit * cluster {
                    // The excess of the last unit is cut off at the Data length.
                    trim_total(&mut out, d.size);
                }
            } else {
                out.push(Piece::Lznt {
                    off: v.abs(used[0] * cluster),
                    clen: used.len() as u64 * cluster,
                    ulen,
                });
            }
            left -= ulen;
        }
        return out;
    }
    let mut left = d.size;
    let init = d.init.min(d.size);
    let mut written = 0u64;
    for (_, lcn, len) in runs {
        if left == 0 {
            break;
        }
        let bytes = (len * cluster).min(left);
        match lcn {
            Some(l) => {
                // The uninitialized region (past init) must read as zero.
                let valid = bytes.min(init.saturating_sub(written));
                if valid > 0 {
                    out.push(Piece::Data { off: v.abs(l * cluster), len: valid });
                }
                if bytes > valid {
                    out.push(Piece::Zero { len: bytes - valid });
                }
            }
            None => out.push(Piece::Zero { len: bytes }),
        }
        written += bytes;
        left -= bytes;
    }
    out
}

fn trim_total(p: &mut [Piece], total: u64) {
    let sum: u64 = p.iter().map(|x| x.out_len()).sum();
    if sum > total {
        if let Some(Piece::Data { len, .. }) = p.last_mut() {
            *len -= (sum - total).min(*len);
        }
    }
}

struct Bitmap {
    bits: Vec<u8>,
}

impl Bitmap {
    fn used(&self, c: u64) -> bool {
        self.bits.get((c / 8) as usize).map(|b| b & (1 << (c % 8)) != 0).unwrap_or(false)
    }
}

/// Collects FILE_NAME records in a directory index (and its unused part).
/// ntfs3 removes $FILE_NAME from the record on delete; the name stays here.
fn index_names(b: &[u8], dir: u64, out: &mut HashMap<u64, (u64, String)>) {
    let mut p = 0usize;
    while p + 16 + 66 <= b.len() {
        let child = u64le(b, p) & 0xFFFF_FFFF_FFFF;
        let parent = u64le(b, p + 16) & 0xFFFF_FFFF_FFFF;
        if parent == dir && child != 0 {
            let n = u8_at(b, p + 16 + 64) as usize;
            let ns = u8_at(b, p + 16 + 65);
            let key = u16le(b, p + 10) as usize;
            if n > 0 && ns <= 3 && key == 66 + n * 2 {
                if let Some(nb) = b.get(p + 16 + 66..p + 16 + 66 + n * 2) {
                    let name = utf16le_string(nb);
                    let clean = !name.chars().any(|c| (c as u32) < 32 || c == '\u{FFFD}');
                    if clean && (ns != 2 || !out.contains_key(&child)) {
                        out.insert(child, (dir, name));
                    }
                }
            }
        }
        p += 8;
    }
}

/// How much of the clusters in a file's extents went to another file?
fn chance_of(d: &DataAttr, bm: &Option<Bitmap>) -> u8 {
    let Some(bm) = bm else { return 80 };
    if d.resident.is_some() {
        return 98;
    }
    let (mut tot, mut used) = (0u64, 0u64);
    for (_, lcn, len) in &d.runs {
        if let Some(l) = lcn {
            for k in 0..(*len).min(1 << 20) {
                tot += 1;
                if bm.used(l + k) {
                    used += 1;
                }
            }
        }
    }
    if tot == 0 {
        return 60;
    }
    let f = used as f64 / tot as f64;
    if f == 0.0 {
        95
    } else if f < 0.1 {
        55
    } else if f < 0.6 {
        25
    } else {
        4
    }
}

struct Scan<'a> {
    v: &'a Vol,
    ctx: &'a Ctx,
    vol: u16,
    root: u32,
    cluster: u64,
    recs: HashMap<u64, (String, u64, u16, u16, bool)>, // name, parent, parent seq, own seq, deleted
    dirmap: HashMap<u64, u32>,
}

impl<'a> Scan<'a> {
    fn dir_for(&mut self, no: u64, depth: u32) -> u32 {
        if no == ROOT_REC {
            return self.root;
        }
        if let Some(d) = self.dirmap.get(&no) {
            return *d;
        }
        let info = self.recs.get(&no).cloned();
        let id = match info {
            Some((name, parent, pseq, _, del)) if depth < 256 => {
                let valid = match self.recs.get(&parent) {
                    Some(p) => p.3 == pseq || p.3 == pseq.wrapping_add(1) || parent == ROOT_REC,
                    None => parent == ROOT_REC,
                };
                let up = if valid {
                    self.dir_for(parent, depth + 1)
                } else {
                    self.orphan_dir()
                };
                self.ctx.cat.write().unwrap().child_dir(up, &name, del)
            }
            _ => self.orphan_dir(),
        };
        self.dirmap.insert(no, id);
        id
    }
    fn orphan_dir(&self) -> u32 {
        self.ctx.cat.write().unwrap().child_dir(self.root, "$Orphans", true)
    }
}

pub fn scan(v: &Vol, ctx: &Ctx, vol: u16, root: u32) -> Result<FsResult, String> {
    let b = v.read(0, 512);
    let bps = u16le(&b, 11) as u64;
    let spc = b[13] as u64;
    let spc = if spc > 128 { 1u64 << (256 - spc) } else { spc };
    if bps == 0 || spc == 0 {
        return Err("invalid NTFS boot record".into());
    }
    let cluster = bps * spc;
    let mft_lcn = u64le(&b, 0x30);
    let crec = b[0x40] as i8;
    let rec_size = if crec < 0 { 1u64 << (-crec as u32) } else { crec as u64 * cluster };
    if !(512..=65536).contains(&rec_size) {
        return Err("invalid NTFS record size".into());
    }
    let rs = rec_size as usize;
    let mut first = v.read(mft_lcn * cluster, rs);
    let boot_ok = &first[0..4] == b"FILE";
    let mut alloc = Intervals::default();

    let mut runs: Vec<(u64, Option<u64>, u64)> = Vec::new();
    let mut mft_size = 0;
    if boot_ok {
        if let Some(r) = parse_record(&mut first, 0, bps as usize) {
            if let Some(d) = r.data {
                mft_size = d.size;
                runs = d.runs;
            }
        }
    }
    if runs.is_empty() {
        return if ctx.opts.mode == "quick" {
            Err("cannot read $MFT, deep scan needed".into())
        } else {
            sweep(v, ctx, vol, root, cluster, rs, bps as usize)
        };
    }

    ctx.set_phase("phase.mft", mft_size / rec_size);
    let mut raw_recs: Vec<Rec> = Vec::new();
    let mut ext: HashMap<u64, Vec<DataAttr>> = HashMap::new();
    let mut no = 0u64;
    let mut names: HashMap<u64, (String, u64, u16, u16, bool)> = HashMap::new();
    let mut idx_names: HashMap<u64, (u64, String)> = HashMap::new();
    let mut dir_index: Vec<(u64, Vec<(u64, Option<u64>, u64)>)> = Vec::new();
    let mut buf = vec![0u8; 1 << 20];
    'outer: for (_, lcn, len) in &runs {
        let Some(l) = lcn else {
            no += len * cluster / rec_size;
            continue;
        };
        let mut off = l * cluster;
        let end = off + len * cluster;
        ctx.mark(v.abs(off), v.abs(end), CELL_META);
        alloc.push(v.abs(off), v.abs(end));
        while off < end {
            if !ctx.alive() {
                break 'outer;
            }
            let n = ((end - off) as usize).min(buf.len());
            v.read_into(off, &mut buf[..n]);
            for chunk in buf[..n].chunks_exact_mut(rs) {
                if let Some(r) = parse_record(chunk, no, bps as usize) {
                    if r.base != 0 {
                        if let Some(d) = r.data {
                            ext.entry(r.base).or_default().push(d);
                        }
                    } else if !r.name.is_empty() {
                        names.insert(
                            r.no,
                            (r.name.clone(), r.parent, r.parent_seq, r.seq, r.flags & IN_USE == 0),
                        );
                        if r.flags & IS_DIR != 0 {
                            // Small directories stay inside the record.
                            index_names(chunk, r.no, &mut idx_names);
                            if !r.index.is_empty() {
                                dir_index.push((r.no, r.index.clone()));
                            }
                        }
                        raw_recs.push(r);
                    } else if r.flags & IN_USE == 0 && r.no >= 24 && r.data.is_some() {
                        // ntfs3 deleted the name: the content record is still valuable.
                        raw_recs.push(r);
                    }
                }
                no += 1;
            }
            off += n as u64;
            ctx.advance(no);
        }
    }

    // $Bitmap (record 6) gives the used clusters.
    let mut bitmap = None;
    if let Some(r) = raw_recs.iter().find(|r| r.no == 6) {
        if let Some(d) = &r.data {
            let mut bytes = Vec::with_capacity(d.size as usize);
            for (_, lcn, len) in &d.runs {
                match lcn {
                    Some(l) => bytes.extend_from_slice(&v.read(l * cluster, (len * cluster) as usize)),
                    None => bytes.resize(bytes.len() + (len * cluster) as usize, 0),
                }
            }
            bytes.truncate(d.size as usize);
            let bm = Bitmap { bits: bytes };
            let total = v.len / cluster;
            let mut c = 0;
            while c < total {
                if bm.used(c) {
                    let s = c;
                    while c < total && bm.used(c) {
                        c += 1;
                    }
                    alloc.push(v.abs(s * cluster), v.abs(c * cluster));
                } else {
                    c += 1;
                }
            }
            bitmap = Some(bm);
        }
    }
    alloc.normalize();

    if raw_recs.iter().any(|r| r.name.is_empty()) {
        ctx.set_phase("phase.i30", dir_index.len() as u64);
        for (i, (dir, runs)) in dir_index.iter().enumerate() {
            ctx.advance(i as u64);
            for (_, lcn, len) in runs {
                let Some(l) = lcn else { continue };
                let raw = v.read(l * cluster, (len * cluster).min(16 << 20) as usize);
                index_names(&raw, *dir, &mut idx_names);
            }
        }
    }
    let mut s = Scan { v, ctx, vol, root, cluster, recs: names, dirmap: HashMap::new() };
    ctx.set_phase("phase.resolve", raw_recs.len() as u64);
    let mut label = String::new();
    let mut trash_info: HashMap<(u32, String), (String, i64)> = HashMap::new();
    let mut pending: Vec<(Rec, DataAttr, u32)> = Vec::new();
    for (i, mut r) in raw_recs.into_iter().enumerate() {
        if i % 4096 == 0 {
            ctx.advance(i as u64);
            if !ctx.alive() {
                break;
            }
        }
        if r.no == 3 {
            label = String::new();
        }
        if r.no < 24 || r.flags & IS_DIR != 0 {
            continue;
        }
        let deleted = r.flags & IN_USE == 0;
        let mut d = match r.data.take() {
            Some(d) => d,
            None => continue,
        };
        if let Some(list) = ext.get(&r.no) {
            for x in list {
                d.runs.extend(x.runs.iter().cloned());
            }
        }
        if d.size == 0 {
            continue;
        }
        if r.name.is_empty() {
            match idx_names.get(&r.no) {
                Some((parent, name)) => {
                    r.name = name.clone();
                    r.parent = *parent;
                    r.parent_seq = s.recs.get(parent).map(|p| p.3).unwrap_or(0);
                }
                None => {
                    r.name = format!("inode_{}", r.no);
                    r.parent = u64::MAX;
                }
            }
        }
        let dir = dir_of(&mut s, &r);
        // $Recycle.Bin: the $I file holds the original path, the $R file the content.
        if !deleted && r.name.starts_with("$I") && d.resident.is_some() {
            if let Some(res) = &d.resident {
                let ver = u64le(res, 0);
                let del = filetime(u64le(res, 16));
                let path = if ver == 2 {
                    let n = u32le(res, 24) as usize;
                    res.get(28..28 + n * 2).map(utf16le_string)
                } else {
                    res.get(24..24 + 520).map(utf16le_string)
                };
                if let Some(p) = path {
                    let p = p.trim_end_matches('\0').to_string();
                    trash_info.insert((dir, r.name[2..].to_string()), (p, del));
                }
            }
            continue;
        }
        pending.push((r, d, dir));
    }

    for (r, d, dir) in pending {
        let deleted = r.flags & IN_USE == 0;
        let tkey = r.name.strip_prefix("$R").map(|t| (dir, t.to_string()));
        let trash = tkey.and_then(|k| trash_info.get(&k).cloned());
        let live_trash = !deleted && s.ctx.cat.read().unwrap().in_trash(dir);
        if !deleted && trash.is_none() && !live_trash && !s.ctx.opts.existing {
            continue;
        }
        let (name, state, dir, dtime) = match trash {
            Some((path, del)) => {
                let n = path.rsplit(['\\', '/']).next().unwrap_or(&r.name).to_string();
                let tdir = s.ctx.cat.write().unwrap().child_dir(s.root, "$Recycle Bin", false);
                (n, State::Trash, tdir, del)
            }
            None => (
                r.name.clone(),
                if deleted { State::Deleted } else { State::Existing },
                dir,
                0,
            ),
        };
        let mut e = Entry::new(s.vol, dir, name, d.size, state, "NTFS MFT");
        e.mtime = r.mtime;
        e.ctime = r.ctime;
        e.dtime = dtime;
        e.chance = if deleted { chance_of(&d, &bitmap) } else { 100 };
        if d.encrypted {
            e.chance = e.chance.min(10);
            e.note = Some("note.efs".into());
        }
        e.pieces = pieces_of(&d, s.cluster, s.v);
        s.ctx.add(e);
    }

    Ok(FsResult { fstype: "NTFS".into(), label, block_size: cluster as u32, alloc, note: None })
}

fn dir_of(s: &mut Scan, r: &Rec) -> u32 {
    let parent_ok = match s.recs.get(&r.parent) {
        Some(p) => p.3 == r.parent_seq || p.3 == r.parent_seq.wrapping_add(1) || r.parent == ROOT_REC,
        None => r.parent == ROOT_REC,
    };
    if parent_ok {
        s.dir_for(r.parent, 0)
    } else {
        s.orphan_dir()
    }
}

/// Volume with an unreadable $MFT: looks for aligned FILE-signed records.
fn sweep(v: &Vol, ctx: &Ctx, vol: u16, root: u32, cluster: u64, rs: usize, ss: usize) -> Result<FsResult, String> {
    ctx.set_phase("phase.mftsweep", v.len);
    let orphan = ctx.cat.write().unwrap().child_dir(root, "$Orphans", true);
    let mut buf = vec![0u8; 8 << 20];
    let mut off = 0u64;
    let mut names: HashMap<u64, (String, u64, u16, u16, bool)> = HashMap::new();
    let mut found: Vec<(Rec, DataAttr)> = Vec::new();
    while off < v.len {
        if !ctx.alive() {
            break;
        }
        let n = ((v.len - off) as usize).min(buf.len());
        v.read_into(off, &mut buf[..n]);
        for (i, c) in buf[..n].chunks_exact_mut(rs).enumerate() {
            if &c[0..4] != b"FILE" {
                continue;
            }
            let no = u32le(c, 44) as u64;
            if let Some(mut r) = parse_record(c, no, ss) {
                if r.name.is_empty() || r.base != 0 {
                    continue;
                }
                names.insert(no, (r.name.clone(), r.parent, r.parent_seq, r.seq, r.flags & IN_USE == 0));
                if r.flags & IS_DIR != 0 {
                    continue;
                }
                if let Some(d) = r.data.take() {
                    if d.size > 0 {
                        found.push((r, d));
                    }
                }
            }
            let _ = i;
        }
        off += n as u64;
        ctx.advance(off);
    }
    let mut s = Scan { v, ctx, vol, root, cluster, recs: names, dirmap: HashMap::new() };
    for (r, d) in found {
        let deleted = r.flags & IN_USE == 0;
        let dir = dir_of(&mut s, &r);
        let dir = if dir == root { orphan } else { dir };
        let mut e = Entry::new(vol, dir, r.name.clone(), d.size, if deleted { State::Deleted } else { State::Orphan }, "NTFS sweep");
        e.mtime = r.mtime;
        e.ctime = r.ctime;
        e.chance = 70;
        e.pieces = pieces_of(&d, cluster, v);
        ctx.add(e);
    }
    Ok(FsResult {
        fstype: "NTFS".into(),
        label: String::new(),
        block_size: cluster as u32,
        alloc: Intervals::default(),
        note: Some("note.mft_bad".into()),
    })
}
