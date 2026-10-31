//! Rebuilds the content of a found record from its pieces.

use crate::codec;
use crate::model::*;
use crate::source::Source;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::FileExt;

const IO: usize = 1 << 20;

/// Streams the pieces to the output in order. Returns the number of bytes written.
pub fn stream(src: &Source, e: &Entry, mut out: impl FnMut(&[u8]) -> bool, limit: u64) -> u64 {
    let mut written = 0u64;
    let mut buf = vec![0u8; IO];
    let want = e.size.min(limit);
    for p in &e.pieces {
        if written >= want {
            break;
        }
        let left = want - written;
        match p {
            Piece::Data { off, len } => {
                let total = (*len).min(left);
                let mut o = 0u64;
                while o < total {
                    let n = ((total - o) as usize).min(IO);
                    src.read_at(off + o, &mut buf[..n]);
                    if !out(&buf[..n]) {
                        return written;
                    }
                    o += n as u64;
                    written += n as u64;
                }
            }
            Piece::Zero { len } => {
                let total = (*len).min(left);
                let mut o = 0u64;
                buf[..IO.min(total as usize)].fill(0);
                while o < total {
                    let n = ((total - o) as usize).min(IO);
                    if !out(&buf[..n]) {
                        return written;
                    }
                    o += n as u64;
                    written += n as u64;
                }
            }
            Piece::Inline { data } => {
                let n = (data.len() as u64).min(left) as usize;
                if !out(&data[..n]) {
                    return written;
                }
                written += n as u64;
            }
            Piece::Comp { off, clen, codec: c, ulen, skip, len } => {
                let raw = src.read_vec(*off, *clen as usize);
                let total = (*len).min(left) as usize;
                let plain = codec::decompress(*c, &raw, *ulen as usize, src.sector.max(4096) as usize);
                match plain {
                    Some(d) if d.len() as u64 >= skip + total as u64 => {
                        if !out(&d[*skip as usize..*skip as usize + total]) {
                            return written;
                        }
                    }
                    other => {
                        // Partial decompression: pad the rest with zeros so offsets do not shift.
                        let mut v = other.unwrap_or_default();
                        v.resize(*skip as usize + total, 0);
                        if !out(&v[*skip as usize..*skip as usize + total]) {
                            return written;
                        }
                    }
                }
                written += total as u64;
            }
            Piece::Lznt { off, clen, ulen } => {
                let raw = src.read_vec(*off, *clen as usize);
                let d = codec::lznt1(&raw, *ulen as usize);
                let n = (*ulen).min(left) as usize;
                if !out(&d[..n]) {
                    return written;
                }
                written += n as u64;
            }
            Piece::Host { path, len } => {
                let Ok(f) = File::open(path) else { continue };
                let total = (*len).min(left);
                let mut o = 0u64;
                while o < total {
                    let n = ((total - o) as usize).min(IO);
                    if f.read_exact_at(&mut buf[..n], o).is_err() {
                        return written;
                    }
                    if !out(&buf[..n]) {
                        return written;
                    }
                    o += n as u64;
                    written += n as u64;
                }
            }
        }
    }
    written
}

pub fn head(src: &Source, e: &Entry, n: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(n.min(1 << 20));
    stream(
        src,
        e,
        |b| {
            let room = n - v.len();
            v.extend_from_slice(&b[..room.min(b.len())]);
            v.len() < n
        },
        n as u64,
    );
    v
}

/// Specific range: jumps straight to the target piece without reading earlier ones.
pub fn range(src: &Source, e: &Entry, off: u64, n: usize) -> Vec<u8> {
    let mut v: Vec<u8> = Vec::with_capacity(n);
    let end = off.saturating_add(n as u64).min(e.size);
    let mut pos = 0u64;
    for p in &e.pieces {
        if pos >= end {
            break;
        }
        let plen = p.out_len();
        if pos + plen <= off {
            pos += plen;
            continue;
        }
        let s = off.max(pos) - pos;
        let t = (end - pos).min(plen) - s;
        match p {
            Piece::Data { off: o, .. } => {
                let at = v.len();
                v.resize(at + t as usize, 0);
                src.read_at(o + s, &mut v[at..]);
            }
            Piece::Zero { .. } => v.resize(v.len() + t as usize, 0),
            Piece::Inline { data } => v.extend_from_slice(&data[s as usize..(s + t) as usize]),
            Piece::Comp { off: o, clen, codec: c, ulen, skip, .. } => {
                let raw = src.read_vec(*o, *clen as usize);
                let d = codec::decompress(*c, &raw, *ulen as usize, src.sector.max(4096) as usize).unwrap_or_default();
                let a = (*skip + s) as usize;
                let b = (a + t as usize).min(d.len());
                if a < b {
                    v.extend_from_slice(&d[a..b]);
                }
                v.resize(v.len() + (a + t as usize - b.max(a)), 0);
            }
            Piece::Lznt { off: o, clen, ulen } => {
                let raw = src.read_vec(*o, *clen as usize);
                let d = codec::lznt1(&raw, *ulen as usize);
                v.extend_from_slice(&d[s as usize..(s + t) as usize]);
            }
            Piece::Host { path, .. } => {
                let at = v.len();
                v.resize(at + t as usize, 0);
                if let Ok(f) = File::open(path) {
                    let _ = f.read_exact_at(&mut v[at..], s);
                }
            }
        }
        pos += plen;
    }
    v
}

pub fn to_file(src: &Source, e: &Entry, path: &std::path::Path, limit: u64) -> std::io::Result<u64> {
    let mut f = File::create(path)?;
    let mut err = None;
    let n = stream(
        src,
        e,
        |b| match f.write_all(b) {
            Ok(()) => true,
            Err(x) => {
                err = Some(x);
                false
            }
        },
        limit,
    );
    match err {
        Some(x) => Err(x),
        None => Ok(n),
    }
}
