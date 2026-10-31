//! File system decompressors: btrfs (zlib, lzo, zstd) and NTFS LZNT1.

use std::io::Read;

pub const ZLIB: u8 = 1;
pub const LZO: u8 = 2;
pub const ZSTD: u8 = 3;

pub fn decompress(codec: u8, input: &[u8], ulen: usize, sector: usize) -> Option<Vec<u8>> {
    match codec {
        ZLIB => {
            let mut out = Vec::with_capacity(ulen);
            let mut d = flate2::read::ZlibDecoder::new(input);
            let _ = d.read_to_end(&mut out);
            (!out.is_empty()).then_some(out)
        }
        ZSTD => {
            let mut out = Vec::with_capacity(ulen);
            let d = zstd::stream::read::Decoder::with_buffer(input).ok()?;
            let mut d = d.single_frame();
            let _ = d.read_to_end(&mut out);
            (!out.is_empty()).then_some(out)
        }
        LZO => btrfs_lzo(input, ulen, sector),
        _ => None,
    }
}

/// btrfs LZO: 4-byte total length, then chunks that do not cross a sector boundary.
fn btrfs_lzo(input: &[u8], ulen: usize, sector: usize) -> Option<Vec<u8>> {
    if input.len() < 4 {
        return None;
    }
    let total = (u32::from_le_bytes(input[0..4].try_into().ok()?) as usize).min(input.len());
    let mut out = Vec::with_capacity(ulen);
    let mut p = 4usize;
    while p + 4 <= total && out.len() < ulen {
        let room = sector - (p % sector);
        if room < 4 {
            p += room;
            continue;
        }
        let seg = u32::from_le_bytes(input[p..p + 4].try_into().ok()?) as usize;
        p += 4;
        if seg == 0 || p + seg > input.len() {
            break;
        }
        if !lzo1x(&input[p..p + seg], &mut out) {
            return None;
        }
        p += seg;
    }
    Some(out)
}

fn zero_run(src: &[u8], ip: &mut usize, base: usize) -> Option<usize> {
    let start = *ip;
    while *src.get(*ip)? == 0 {
        *ip += 1;
    }
    let n = *ip - start;
    let v = *src.get(*ip)? as usize;
    *ip += 1;
    Some(n * 255 + base + v)
}

/// LZO1X decompressor (follows Linux lzo1x_decompress_safe).
pub fn lzo1x(src: &[u8], out: &mut Vec<u8>) -> bool {
    lzo1x_inner(src, out).is_some()
}

fn lzo1x_inner(src: &[u8], out: &mut Vec<u8>) -> Option<()> {
    let base = out.len();
    let mut ip = 0usize;
    let mut state: usize = 0;
    let mut first = true;
    loop {
        let mut t: usize;
        let m_pos: usize;
        let next: usize;
        if first && *src.first()? > 17 {
            first = false;
            let t0 = src[0] as usize - 17;
            ip = 1;
            if t0 < 4 {
                out.extend_from_slice(src.get(ip..ip + t0)?);
                ip += t0;
                state = t0;
                continue;
            }
            out.extend_from_slice(src.get(ip..ip + t0)?);
            ip += t0;
            state = 4;
            continue;
        }
        first = false;
        t = *src.get(ip)? as usize;
        ip += 1;
        if t < 16 {
            if state == 0 {
                if t == 0 {
                    t = zero_run(src, &mut ip, 15)?;
                }
                t += 3;
                out.extend_from_slice(src.get(ip..ip + t)?);
                ip += t;
                state = 4;
                continue;
            } else if state != 4 {
                next = t & 3;
                let b = *src.get(ip)? as usize;
                ip += 1;
                let dist = 1 + (t >> 2) + (b << 2);
                let pos = out.len().checked_sub(dist)?;
                if pos < base {
                    return None;
                }
                let (a, c) = (out[pos], out[pos + 1]);
                out.push(a);
                out.push(c);
                out.extend_from_slice(src.get(ip..ip + next)?);
                ip += next;
                state = next;
                continue;
            } else {
                next = t & 3;
                let b = *src.get(ip)? as usize;
                ip += 1;
                let dist = 1 + 0x0800 + (t >> 2) + (b << 2);
                m_pos = out.len().checked_sub(dist)?;
                t = 3;
            }
        } else if t >= 64 {
            next = t & 3;
            let b = *src.get(ip)? as usize;
            ip += 1;
            let dist = 1 + ((t >> 2) & 7) + (b << 3);
            m_pos = out.len().checked_sub(dist)?;
            t = (t >> 5) + 1;
        } else if t >= 32 {
            t = (t & 31) + 2;
            if t == 2 {
                t = zero_run(src, &mut ip, 31)? + 2;
            }
            let w = u16::from_le_bytes([*src.get(ip)?, *src.get(ip + 1)?]) as usize;
            ip += 2;
            next = w & 3;
            let dist = 1 + (w >> 2);
            m_pos = out.len().checked_sub(dist)?;
        } else {
            let hi = (t & 8) << 11;
            t = (t & 7) + 2;
            if t == 2 {
                t = zero_run(src, &mut ip, 7)? + 2;
            }
            let w = u16::from_le_bytes([*src.get(ip)?, *src.get(ip + 1)?]) as usize;
            ip += 2;
            next = w & 3;
            let d = hi + (w >> 2);
            if d == 0 {
                return Some(());
            }
            m_pos = out.len().checked_sub(d + 0x4000)?;
        }
        if m_pos < base {
            return None;
        }
        for k in 0..t {
            let c = out[m_pos + k];
            out.push(c);
        }
        out.extend_from_slice(src.get(ip..ip + next)?);
        ip += next;
        state = next;
    }
}

/// NTFS LZNT1: 4 KiB chunks, each raw or tokenized.
pub fn lznt1(input: &[u8], ulen: usize) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(ulen);
    let mut p = 0usize;
    while p + 2 <= input.len() && out.len() < ulen {
        let hdr = u16::from_le_bytes([input[p], input[p + 1]]);
        if hdr == 0 {
            break;
        }
        p += 2;
        let size = (hdr & 0xFFF) as usize + 1;
        let end = (p + size).min(input.len());
        if hdr & 0x8000 == 0 {
            out.extend_from_slice(&input[p..end]);
            p = end;
            continue;
        }
        let chunk_start = out.len();
        while p < end {
            let flags = input[p];
            p += 1;
            for bit in 0..8 {
                if p >= end {
                    break;
                }
                if flags & (1 << bit) == 0 {
                    out.push(input[p]);
                    p += 1;
                } else {
                    if p + 2 > end {
                        p = end;
                        break;
                    }
                    let tok = u16::from_le_bytes([input[p], input[p + 1]]) as usize;
                    p += 2;
                    let pos = out.len() - chunk_start;
                    let mut l = 12u32;
                    let mut i = pos.saturating_sub(1);
                    while i >= 0x10 {
                        l -= 1;
                        i >>= 1;
                    }
                    let len = (tok & ((1 << l) - 1)) + 3;
                    let dist = (tok >> l) + 1;
                    if dist > out.len() {
                        return out;
                    }
                    let start = out.len() - dist;
                    for k in 0..len {
                        let c = out[start + k];
                        out.push(c);
                    }
                }
            }
        }
    }
    // If the chunk ended early, the rest of the unit is zero.
    out.resize(ulen, 0);
    out
}
