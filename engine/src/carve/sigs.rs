//! Format parsers: instead of checking only the signature, they walk the file structure
//! to find the real size. This is also what reduces false positives.

use crate::model::Kind;
use crate::source::*;
use std::io::{BufRead, Read};

const MB: u64 = 1 << 20;
const GB: u64 = 1 << 30;

pub struct Hit {
    pub ext: &'static str,
    pub size: u64,
    pub complete: bool,
    pub title: Option<String>,
    pub mtime: i64,
    /// Fragmented file: list of (offset, length) relative to the start.
    pub parts: Option<Vec<(u64, u64)>>,
    pub repaired: bool,
}

impl Hit {
    fn new(ext: &'static str, size: u64, complete: bool) -> Option<Hit> {
        Some(Hit { ext, size, complete, title: None, mtime: 0, parts: None, repaired: false })
    }
}

/// Stream view over the source, starting at base.
pub struct R<'a> {
    pub src: &'a Source,
    pub base: u64,
    pub limit: u64,
}

impl<'a> R<'a> {
    pub fn new(src: &'a Source, base: u64, limit: u64) -> R<'a> {
        R { src, base, limit }
    }

    fn get(&self, off: u64, n: usize) -> Vec<u8> {
        if off >= self.limit {
            return vec![0; n];
        }
        self.src.read_vec(self.base + off, n)
    }

    /// Scans forward until `f(chunk, offset)` returns true.
    fn scan(&self, start: u64, cap: u64, mut f: impl FnMut(&[u8], u64) -> bool) {
        const STEP: usize = 1 << 20;
        let mut o = start;
        let end = (start + cap).min(self.limit);
        let mut buf = vec![0u8; STEP + 16];
        while o < end {
            let n = ((end - o) as usize).min(STEP + 16);
            self.src.read_at(self.base + o, &mut buf[..n]);
            if f(&buf[..n], o) {
                return;
            }
            o += STEP as u64;
        }
    }

    /// Finds the end of formats with no stored size: stops at a zero-filled block or
    /// at an aligned new file signature, and trims the trailing padding.
    fn until_gap(&self, start: u64, cap: u64) -> u64 {
        let mut end = start;
        self.scan(start, cap, |b, o| {
            let abs = self.base + o;
            let skip = (4096 - (abs % 4096)) as usize % 4096;
            if o == start {
                end = o + (skip as u64).min(b.len() as u64);
            }
            for (i, s) in b.get(skip..).unwrap_or(&[]).chunks(4096).enumerate() {
                let at = o + skip as u64 + (i as u64) * 4096;
                if at <= start {
                    continue;
                }
                if s.iter().all(|c| *c == 0) || sniff(s).is_some() {
                    end = at;
                    return true;
                }
                end = at + s.len() as u64;
            }
            false
        });
        let end = end.min(start + cap).min(self.limit);
        // Drop padding zeros from the last block.
        let tail_start = end.saturating_sub(4096).max(start);
        let tail = self.get(tail_start, (end - tail_start) as usize);
        let keep = tail.iter().rposition(|c| *c != 0).map(|p| p + 1).unwrap_or(0);
        tail_start + keep as u64
    }
}

pub fn first_byte_table() -> [bool; 256] {
    let mut t = [false; 256];
    for b in [
        0xFF, 0x89, b'G', b'B', b'I', b'M', b'R', 0x1A, b'f', b'F', b'O', b'%', b'P', 0x37, 0x1F, 0xFD,
        0x28, 0x7F, b'S', 0xD0, b'8', 0x30, 0x00, b'{', b'<', b'#', b'-', b'!', 0x01, b'w', 0x47,
    ] {
        t[b as usize] = true;
    }
    t
}

pub fn probe(h: &[u8], r: &R) -> Option<Hit> {
    if h.len() < 600 {
        return None;
    }
    // The ftyp/moov signature is at byte 4, the first byte tells nothing.
    if &h[4..8] == b"ftyp" {
        return bmff(h, r);
    }
    if matches!(&h[4..8], b"moov" | b"wide" | b"mdat") && u32be(h, 0) > 7 {
        let first = r.get(u32be(h, 0) as u64, 16);
        if matches!(&first[4..8], b"moov" | b"mdat" | b"free" | b"skip" | b"wide") {
            return bmff(h, r);
        }
    }
    match h[0] {
        0xFF if h[1] == 0xD8 && h[2] == 0xFF => jpeg(r),
        0xFF if h[1] & 0xE0 == 0xE0 => mp3(r, 0),
        0x89 if &h[1..8] == b"PNG\r\n\x1a\n" => png(r),
        b'G' if &h[..6] == b"GIF89a" || &h[..6] == b"GIF87a" => gif(r),
        b'G' if h[0] == 0x47 && h[188] == 0x47 && h[376] == 0x47 && h[564] == 0x47 => mpegts(r),
        b'B' if &h[..2] == b"BM" => bmp(h),
        b'B' if &h[..3] == b"BZh" && (b'1'..=b'9').contains(&h[3]) && h[4..10] == [0x31, 0x41, 0x59, 0x26, 0x53, 0x59] => {
            Hit::new("bz2", r.until_gap(0, 4 * GB), false)
        }
        b'B' if h.starts_with(b"BEGIN:VCARD") => text(r, "vcf"),
        b'B' if h.starts_with(b"BEGIN:VCALENDAR") => text(r, "ics"),
        b'I' if h[1] == b'I' && (h[2] == 0x2A || h[2] == b'R' || h[2] == b'U') && h[3] == 0 => tiff(h, r, true),
        b'M' if h[1] == b'M' && h[2] == 0 && h[3] == 0x2A => tiff(h, r, false),
        b'I' if &h[..3] == b"ID3" => id3(h, r),
        b'M' if h[1] == b'Z' => pe(h, r),
        b'M' if &h[..4] == b"MThd" => midi(r),
        b'R' if &h[..4] == b"RIFF" => riff(h),
        b'R' if h.starts_with(b"Rar!\x1a\x07") => rar(h, r),
        0x1A if h[1..4] == [0x45, 0xDF, 0xA3] => ebml(h, r),
        b'f' if &h[..4] == b"fLaC" => flac(r),
        b'F' if &h[..3] == b"FLV" && h[3] == 1 => flv(h, r),
        b'F' if &h[..4] == b"FORM" && (&h[8..12] == b"AIFF" || &h[8..12] == b"AIFC") => {
            Hit::new("aiff", u32be(h, 4) as u64 + 8, true)
        }
        b'O' if &h[..4] == b"OggS" && h[4] == 0 => ogg(h, r),
        b'O' if &h[..4] == b"OTTO" => font(h, r),
        b'%' if &h[..5] == b"%PDF-" => pdf(r),
        b'P' if &h[..4] == b"PK\x03\x04" => zip(r),
        0x37 if h[1..6] == [0x7A, 0xBC, 0xAF, 0x27, 0x1C] => {
            Hit::new("7z", 32 + u64le(h, 12) + u64le(h, 20), true)
        }
        0x1F if h[1] == 0x8B && h[2] == 8 && h[3] & 0xE0 == 0 => gzip(r),
        0xFD if h[1..6] == [0x37, 0x7A, 0x58, 0x5A, 0x00] => Hit::new("xz", r.until_gap(0, 8 * GB), false),
        0x28 if h[1..4] == [0xB5, 0x2F, 0xFD] => zstd_frames(r),
        0x7F if &h[1..4] == b"ELF" => elf(h),
        b'S' if h.starts_with(b"SQLite format 3\0") => sqlite(h),
        0xD0 if h[1..8] == [0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1] => ole(h, r),
        b'8' if &h[..4] == b"8BPS" && u16be(h, 4) == 1 => psd(h, r),
        0x30 if h[1..8] == [0x26, 0xB2, 0x75, 0x8E, 0x66, 0xCF, 0x11] => asf(h, r),
        0x00 if h[1..4] == [1, 0, 0] && u16be(h, 4) <= 64 && u16be(h, 4) > 4 => font(h, r),
        0x00 if h[1..4] == [0, 1, 0xBA] => Hit::new("mpg", r.until_gap(0, 8 * GB), false),
        0x01 if &h[1..6] == b"CD001" => None,
        b'w' if &h[..4] == b"wOFF" || &h[..4] == b"wOF2" => Hit::new("woff", u32be(h, 8) as u64, true),
        b'{' if h.starts_with(b"{\\rtf1") => text(r, "rtf"),
        b'<' if starts_ci(h, b"<!doctype html") || starts_ci(h, b"<html") => text(r, "html"),
        b'<' if h.starts_with(b"<?xml") => {
            let t = String::from_utf8_lossy(&h[..256.min(h.len())]);
            text(r, if t.contains("<svg") { "svg" } else { "xml" })
        }
        b'<' if h.starts_with(b"<svg") => text(r, "svg"),
        b'#' if h.starts_with(b"#!/") => text(r, "sh"),
        b'-' if h.starts_with(b"-----BEGIN ") => text(r, "pem"),
        b'!' if h.starts_with(b"!<arch>\n") => ar(r),
        _ => None,
    }
}

/// The "ustar" signature of a tar header is at byte 257.
pub fn probe_tar(h: &[u8], r: &R) -> Option<Hit> {
    if h.len() < 512 || &h[257..262] != b"ustar" {
        return None;
    }
    let mut o = 0u64;
    let mut zero = 0;
    loop {
        let b = r.get(o, 512);
        if b.iter().all(|c| *c == 0) {
            zero += 1;
            o += 512;
            if zero == 2 {
                return Hit::new("tar", o, true);
            }
            continue;
        }
        zero = 0;
        if &b[257..262] != b"ustar" {
            return Hit::new("tar", o, false);
        }
        let s = String::from_utf8_lossy(&b[124..135]).trim_matches(|c| c == '\0' || c == ' ').to_string();
        let size = u64::from_str_radix(&s, 8).ok()?;
        o += 512 + size.div_ceil(512) * 512;
        if o > 512 * GB {
            return None;
        }
    }
}

fn starts_ci(h: &[u8], p: &[u8]) -> bool {
    h.len() >= p.len() && h[..p.len()].iter().zip(p).all(|(a, b)| a.to_ascii_lowercase() == *b)
}

fn text(r: &R, ext: &'static str) -> Option<Hit> {
    let mut end = 0u64;
    r.scan(0, 64 * MB, |b, o| {
        for (i, c) in b.iter().enumerate() {
            if *c == 0 || (*c < 9) || (*c > 13 && *c < 32 && *c != 27) {
                end = o + i as u64;
                return true;
            }
        }
        end = o + b.len() as u64;
        false
    });
    if end < 32 {
        return None;
    }
    Hit::new(ext, end, true)
}

fn exif_date(seg: &[u8]) -> i64 {
    // Looks for the first date string in the form "YYYY:MM:DD HH:MM:SS\0".
    for w in seg.windows(20) {
        if w[4] == b':' && w[7] == b':' && w[10] == b' ' && w[13] == b':' && w[16] == b':' && w[19] == 0 && w[..4].iter().all(u8::is_ascii_digit) {
            let n = |a: usize, b: usize| std::str::from_utf8(&w[a..b]).ok().and_then(|s| s.parse::<i64>().ok());
            if let (Some(y), Some(m), Some(d), Some(h), Some(mi), Some(s)) =
                (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16), n(17, 19))
            {
                if (1990..2100).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d) {
                    return crate::fs::fat::days_from_civil(y, m, d) * 86400 + h * 3600 + mi * 60 + s;
                }
            }
        }
    }
    0
}

fn jpeg(r: &R) -> Option<Hit> {
    let mut o = 2u64;
    let mut mtime = 0;
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > 512 {
            return None;
        }
        let m = r.get(o, 4);
        if m[0] != 0xFF {
            return None;
        }
        let t = m[1];
        if t == 0xD9 {
            return Some(Hit { ext: "jpg", size: o + 2, complete: true, title: None, mtime, parts: None, repaired: false });
        }
        if t == 0xFF {
            o += 1;
            continue;
        }
        if t == 0xDA {
            break;
        }
        if (0xD0..=0xD7).contains(&t) || t == 0 {
            return None;
        }
        let len = u16be(&m, 2) as u64;
        if len < 2 {
            return None;
        }
        if t == 0xE1 && mtime == 0 {
            let seg = r.get(o + 4, (len as usize).min(65535));
            if seg.starts_with(b"Exif") {
                mtime = exif_date(&seg);
            }
        }
        o += 2 + len;
    }
    // Entropy data: move on to the next real marker.
    let start = o + 2;
    let mut end = 0u64;
    let mut prev = 0u8;
    let mut complete = false;
    r.scan(start, 256 * MB, |b, off| {
        let n = b.len().min(1 << 20);
        for (i, c) in b[..n].iter().enumerate() {
            if prev == 0xFF {
                match *c {
                    0xD9 => {
                        end = off + i as u64 + 1;
                        complete = true;
                        return true;
                    }
                    0x00 | 0xFF | 0xD0..=0xD7 => {}
                    0xC4 | 0xDA | 0xDB | 0xDD | 0xFE | 0xE0..=0xEF => {} // progressive JPEG
                    _ => {
                        end = off + i as u64 - 1;
                        return true;
                    }
                }
            }
            prev = *c;
        }
        end = off + n as u64;
        false
    });
    if end < 256 {
        return None;
    }
    Some(Hit { ext: "jpg", size: end, complete, title: None, mtime, parts: None, repaired: false })
}

fn png(r: &R) -> Option<Hit> {
    let mut o = 8u64;
    let mut first = true;
    loop {
        let c = r.get(o, 8);
        let len = u32be(&c, 0) as u64;
        let ty = &c[4..8];
        if len > 1 << 31 || !ty.iter().all(u8::is_ascii_alphabetic) {
            return if first { None } else { Hit::new("png", o, false) };
        }
        if first && ty != b"IHDR" {
            return None;
        }
        first = false;
        o += 12 + len;
        if ty == b"IEND" {
            return Hit::new("png", o, true);
        }
    }
}

fn gif(r: &R) -> Option<Hit> {
    let h = r.get(0, 13);
    let mut o = 13u64;
    if h[10] & 0x80 != 0 {
        o += 3 << ((h[10] & 7) + 1);
    }
    loop {
        let b = r.get(o, 12);
        match b[0] {
            0x3B => return Hit::new("gif", o + 1, true),
            0x21 => {
                o += 2;
                loop {
                    let s = r.get(o, 1)[0] as u64;
                    o += 1 + s;
                    if s == 0 {
                        break;
                    }
                }
            }
            0x2C => {
                o += 10;
                if b[9] & 0x80 != 0 {
                    o += 3 << ((b[9] & 7) + 1);
                }
                o += 1;
                loop {
                    let s = r.get(o, 1)[0] as u64;
                    o += 1 + s;
                    if s == 0 {
                        break;
                    }
                }
            }
            _ => return if o > 64 { Hit::new("gif", o, false) } else { None },
        }
        if o > 2 * GB {
            return None;
        }
    }
}

fn bmp(h: &[u8]) -> Option<Hit> {
    let size = u32le(h, 2) as u64;
    let dib = u32le(h, 14);
    let off = u32le(h, 10);
    let w = u32le(h, 18);
    let ht = u32le(h, 22);
    let planes = u16le(h, 26);
    let bpp = u16le(h, 28);
    if !matches!(dib, 12 | 40 | 52 | 56 | 108 | 124) || planes != 1 || !matches!(bpp, 1 | 4 | 8 | 16 | 24 | 32) {
        return None;
    }
    if u32le(h, 6) != 0 || off < 26 || off as u64 > size || w == 0 || w > 65536 || ht == 0 || size < 64 {
        return None;
    }
    Hit::new("bmp", size, true)
}

fn tiff(h: &[u8], r: &R, le: bool) -> Option<Hit> {
    let u16x = |b: &[u8], o| if le { u16le(b, o) } else { u16be(b, o) };
    let u32x = |b: &[u8], o| if le { u32le(b, o) } else { u32be(b, o) };
    let mut ext = "tif";
    if &h[8..10] == b"CR" {
        ext = "cr2";
    } else if h[2] == b'R' {
        ext = "orf";
    } else if h[2] == b'U' {
        ext = "rw2";
    }
    let mut stack = vec![u32x(h, 4) as u64];
    let mut seen = 0;
    let mut end = 8u64;
    while let Some(ifd) = stack.pop() {
        seen += 1;
        if seen > 64 || ifd < 8 || ifd > 2 * GB {
            continue;
        }
        let c = r.get(ifd, 2);
        let n = u16x(&c, 0) as usize;
        if n == 0 || n > 4096 {
            if seen == 1 {
                return None;
            }
            continue;
        }
        let t = r.get(ifd + 2, n * 12 + 4);
        end = end.max(ifd + 6 + n as u64 * 12);
        let (mut strips, mut tiles): ((Vec<u64>, Vec<u64>), (Vec<u64>, Vec<u64>)) = Default::default();
        let mut jpeg = (0u64, 0u64);
        let tsize = |ty: u16| match ty {
            1 | 2 | 6 | 7 => 1u64,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => 0,
        };
        for i in 0..n {
            let e = &t[i * 12..i * 12 + 12];
            let tag = u16x(e, 0);
            let ty = u16x(e, 2);
            let cnt = u32x(e, 4) as u64;
            let val = u32x(e, 8) as u64;
            let bytes = tsize(ty).saturating_mul(cnt);
            if bytes > 4 {
                end = end.max(val + bytes);
            }
            let values = |ty: u16, cnt: u64, val: u64| -> Vec<u64> {
                if cnt <= 1 {
                    return vec![val];
                }
                let w = tsize(ty) as usize;
                if w == 0 {
                    return vec![];
                }
                let raw = r.get(val, (cnt.min(65536) as usize) * w);
                raw.chunks_exact(w).map(|c| if w == 2 { u16x(c, 0) as u64 } else { u32x(c, 0) as u64 }).collect()
            };
            match tag {
                0x111 => strips.0 = values(ty, cnt, val),
                0x117 => strips.1 = values(ty, cnt, val),
                0x144 => tiles.0 = values(ty, cnt, val),
                0x145 => tiles.1 = values(ty, cnt, val),
                0x201 => jpeg.0 = val,
                0x202 => jpeg.1 = val,
                0x8769 | 0x14A | 0x927C => stack.push(val),
                0xC612 => ext = "dng",
                0x10F => {
                    let m = r.get(val, 16);
                    if m.starts_with(b"NIKON") {
                        ext = "nef";
                    } else if m.starts_with(b"SONY") {
                        ext = "arw";
                    }
                }
                _ => {}
            }
        }
        // Strips are paired arrays: the largest offset plus the largest length goes past the end.
        for (o, c) in [&strips, &tiles] .iter().map(|p| (&p.0, &p.1)) {
            for (a, b) in o.iter().zip(c.iter()) {
                end = end.max(a + b);
            }
        }
        end = end.max(jpeg.0 + jpeg.1);
        let next = u32x(&t, n * 12) as u64;
        if next != 0 {
            stack.push(next);
        }
    }
    if end < 512 || end > 8 * GB {
        return None;
    }
    Hit::new(ext, end, true)
}

fn riff(h: &[u8]) -> Option<Hit> {
    let size = u32le(h, 4) as u64 + 8;
    let ext = match &h[8..12] {
        b"WEBP" => "webp",
        b"WAVE" => "wav",
        b"AVI " => "avi",
        _ => return None,
    };
    (size > 64).then_some(())?;
    Hit::new(ext, size, true)
}

fn bmff(h: &[u8], r: &R) -> Option<Hit> {
    let brand = &h[8..12];
    let ext = if &h[4..8] != b"ftyp" {
        "mov"
    } else {
        match brand {
            b"heic" | b"heix" | b"mif1" | b"msf1" | b"hevc" | b"heim" | b"heis" => "heic",
            b"avif" | b"avis" => "avif",
            b"M4A " | b"M4B " | b"M4P " => "m4a",
            b"qt  " => "mov",
            b"crx " => "cr3",
            b"3gp4" | b"3gp5" | b"3gp6" | b"3g2a" | b"3gp2" => "3gp",
            _ => "mp4",
        }
    };
    let (mut o, mut moov, mut mdat) = (0u64, false, false);
    let mut count = 0;
    loop {
        let a = r.get(o, 16);
        let mut sz = u32be(&a, 0) as u64;
        let ty = &a[4..8];
        const OK: [&[u8; 4]; 16] = [
            b"ftyp", b"moov", b"mdat", b"free", b"skip", b"wide", b"pnot", b"uuid", b"meta", b"moof",
            b"mfra", b"sidx", b"styp", b"pdin", b"emsg", b"prft",
        ];
        if !OK.iter().any(|x| *x == ty) {
            break;
        }
        moov |= ty == b"moov" || ty == b"meta";
        mdat |= ty == b"mdat" || ty == b"moof";
        if sz == 1 {
            sz = u64be(&a, 8);
        } else if sz == 0 {
            return Hit::new(ext, r.until_gap(o, 32 * GB), moov);
        }
        if sz < 8 || o + sz > r.limit {
            // The file runs past the region: truncated.
            return Hit::new(ext, (o + sz).min(r.limit), false);
        }
        o += sz;
        count += 1;
        if count > 1 << 16 {
            break;
        }
    }
    if count == 0 || o < 32 {
        return None;
    }
    Hit::new(ext, o, moov && (mdat || ext == "heic" || ext == "avif"))
}

fn vint(b: &[u8]) -> Option<(u64, usize, bool)> {
    let f = *b.first()?;
    if f == 0 {
        return None;
    }
    let n = f.leading_zeros() as usize + 1;
    let mut v = (f as u64) & ((1u64 << (8 - n)) - 1);
    let mut all = v == (1u64 << (8 - n)) - 1;
    for c in b.get(1..n)? {
        v = (v << 8) | *c as u64;
        all &= *c == 0xFF;
    }
    Some((v, n, all))
}

fn ebml(h: &[u8], r: &R) -> Option<Hit> {
    let (hsz, n, _) = vint(&h[4..])?;
    if hsz > 256 {
        return None;
    }
    let ext = if h[..64].windows(4).any(|w| w == b"webm") { "webm" } else { "mkv" };
    let seg = 4 + n as u64 + hsz;
    let b = r.get(seg, 16);
    if b[0..4] != [0x18, 0x53, 0x80, 0x67] {
        return None;
    }
    let (sz, k, unknown) = vint(&b[4..])?;
    let body = seg + 4 + k as u64;
    if !unknown {
        return Hit::new(ext, body + sz, true);
    }
    // Live record with no size written: walk the top-level clusters.
    let mut o = body;
    loop {
        let a = r.get(o, 16);
        let idl = a[0].leading_zeros() as usize + 1;
        if idl > 4 {
            break;
        }
        let id = &a[..idl];
        let known: [&[u8]; 8] = [
            &[0x1F, 0x43, 0xB6, 0x75],
            &[0x1C, 0x53, 0xBB, 0x6B],
            &[0x15, 0x49, 0xA9, 0x66],
            &[0x16, 0x54, 0xAE, 0x6B],
            &[0x12, 0x54, 0xC3, 0x67],
            &[0x11, 0x4D, 0x9B, 0x74],
            &[0x10, 0x43, 0xA7, 0x70],
            &[0xEC],
        ];
        if !known.contains(&id) {
            break;
        }
        let Some((s, l, u)) = vint(&a[idl..]) else { break };
        if u {
            break;
        }
        o += (idl + l) as u64 + s;
    }
    Hit::new(ext, o, false)
}

fn id3_text(b: &[u8]) -> Option<String> {
    let enc = *b.first()?;
    let d = &b[1..];
    let s = match enc {
        1 | 2 => {
            let st = if d.starts_with(&[0xFF, 0xFE]) || d.starts_with(&[0xFE, 0xFF]) { 2 } else { 0 };
            utf16le_string(d.get(st..)?)
        }
        3 => String::from_utf8_lossy(d).into_owned(),
        _ => d.iter().map(|c| *c as char).collect(),
    };
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string();
    (!s.is_empty()).then_some(s)
}

fn id3(h: &[u8], r: &R) -> Option<Hit> {
    if h[6..10].iter().any(|c| *c >= 0x80) {
        return None;
    }
    let sz = ((h[6] as u64) << 21) | ((h[7] as u64) << 14) | ((h[8] as u64) << 7) | h[9] as u64;
    let tag = r.get(10, sz.min(4096) as usize);
    let (mut title, mut artist) = (None, None);
    if h[3] >= 3 {
        let mut p = 0;
        while p + 10 < tag.len() {
            let id = &tag[p..p + 4];
            if id[0] == 0 {
                break;
            }
            let n = u32be(&tag, p + 4) as usize;
            if let Some(d) = tag.get(p + 10..p + 10 + n) {
                if id == b"TIT2" {
                    title = id3_text(d);
                } else if id == b"TPE1" {
                    artist = id3_text(d);
                }
            }
            p += 10 + n;
        }
    }
    let mut hit = mp3(r, 10 + sz)?;
    hit.title = match (artist, title) {
        (Some(a), Some(t)) => Some(format!("{a} - {t}")),
        (None, Some(t)) => Some(t),
        _ => None,
    };
    Some(hit)
}

fn mp3(r: &R, start: u64) -> Option<Hit> {
    const BR1: [u32; 16] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0];
    const BR2: [u32; 16] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0];
    const SR: [u32; 4] = [44100, 48000, 32000, 0];
    let mut o = start;
    let mut frames = 0u32;
    let mut buf = r.get(o, 1 << 16);
    let mut bo = o;
    loop {
        if o + 4 > bo + buf.len() as u64 {
            bo = o;
            buf = r.get(o, 1 << 16);
        }
        let p = (o - bo) as usize;
        let (a, b, c) = (buf[p], buf[p + 1], buf[p + 2]);
        if a != 0xFF || b & 0xE0 != 0xE0 {
            break;
        }
        let ver = (b >> 3) & 3;
        let layer = (b >> 1) & 3;
        if ver == 1 || layer != 1 {
            break;
        }
        let bri = (c >> 4) as usize;
        let sri = ((c >> 2) & 3) as usize;
        let pad = ((c >> 1) & 1) as u32;
        let br = if ver == 3 { BR1[bri] } else { BR2[bri] };
        if br == 0 || sri == 3 {
            break;
        }
        let sr = SR[sri] >> if ver == 3 { 0 } else if ver == 2 { 1 } else { 2 };
        let size = if ver == 3 { 144 * br * 1000 / sr + pad } else { 72 * br * 1000 / sr + pad };
        o += size as u64;
        frames += 1;
        if o >= r.limit {
            break;
        }
    }
    if frames < 16 {
        return None;
    }
    if &r.get(o, 3)[..] == b"TAG" {
        o += 128;
    }
    Hit::new("mp3", o, true)
}

fn flac(r: &R) -> Option<Hit> {
    let mut o = 4u64;
    let mut est = 0u64;
    for i in 0..128 {
        let b = r.get(o, 42);
        let last = b[0] & 0x80 != 0;
        let len = ((b[1] as u64) << 16) | ((b[2] as u64) << 8) | b[3] as u64;
        if i == 0 {
            if b[0] & 0x7F != 0 || len != 34 {
                return None;
            }
            let rate = ((b[14] as u64) << 12) | ((b[15] as u64) << 4) | ((b[16] as u64) >> 4);
            let ch = ((b[16] as u64 >> 1) & 7) + 1;
            let bits = (((b[16] as u64 & 1) << 4) | (b[17] as u64 >> 4)) + 1;
            let samples = ((b[17] as u64 & 15) << 32) | u32be(&b, 18) as u64;
            if rate == 0 {
                return None;
            }
            est = samples * ch * bits / 8;
        }
        o += 4 + len;
        if last {
            break;
        }
    }
    let cap = if est > 0 { est + MB } else { 2 * GB };
    Hit::new("flac", r.until_gap(o, cap), false)
}

fn ogg(h: &[u8], r: &R) -> Option<Hit> {
    let ext = if h[..128].windows(8).any(|w| w == b"OpusHead") {
        "opus"
    } else if h[..128].windows(6).any(|w| w == b"theora") {
        "ogv"
    } else {
        "ogg"
    };
    let mut o = 0u64;
    let mut pages = 0;
    loop {
        let b = r.get(o, 27);
        if &b[..4] != b"OggS" {
            break;
        }
        let segs = b[26] as usize;
        let lace = r.get(o + 27, segs);
        o += 27 + segs as u64 + lace.iter().map(|c| *c as u64).sum::<u64>();
        pages += 1;
        if b[5] & 4 != 0 {
            return Hit::new(ext, o, true);
        }
        if o > 8 * GB {
            break;
        }
    }
    if pages < 3 {
        return None;
    }
    Hit::new(ext, o, false)
}

fn find_all(hay: &[u8], needle: &[u8], mut f: impl FnMut(usize)) {
    let first = needle[0];
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        match hay[i..].iter().position(|c| *c == first) {
            None => return,
            Some(p) => {
                i += p;
                if hay[i..].starts_with(needle) {
                    f(i);
                }
                i += 1;
            }
        }
    }
}

fn pdf(r: &R) -> Option<Hit> {
    let mut last = 0u64;
    let mut stop = false;
    r.scan(5, 2 * GB, |b, o| {
        find_all(b, b"%%EOF", |i| {
            // The line ending after %%EOF belongs to the file.
            let mut e = i + 5;
            while e < b.len() && e < i + 7 && matches!(b[e], b'\r' | b'\n') {
                e += 1;
            }
            last = last.max(o + e as u64);
        });
        // If a new aligned PDF starts after %%EOF, the previous file has ended.
        if last > 0 {
            let mut hit = false;
            find_all(b, b"%PDF-", |i| {
                if (o + i as u64 + r.base) % 512 == 0 && o + i as u64 > last {
                    hit = true;
                }
            });
            if hit {
                stop = true;
                return true;
            }
            if o > last + 8 * MB {
                return true;
            }
        }
        for s in b.chunks(4096) {
            if s.len() == 4096 && s.iter().all(|c| *c == 0) {
                return true;
            }
        }
        false
    });
    let _ = stop;
    if last == 0 {
        return Hit::new("pdf", r.until_gap(0, 64 * MB), false);
    }
    Hit::new("pdf", last, true)
}

fn zip(r: &R) -> Option<Hit> {
    let mut o = 0u64;
    let mut ext = "zip";
    let mut count = 0;
    let mut central = false;
    let name_rule = |n: &[u8]| -> Option<&'static str> {
        if n.starts_with(b"word/") {
            Some("docx")
        } else if n.starts_with(b"xl/") {
            Some("xlsx")
        } else if n.starts_with(b"ppt/") {
            Some("pptx")
        } else if n == b"AndroidManifest.xml" {
            Some("apk")
        } else if n == b"META-INF/MANIFEST.MF" {
            Some("jar")
        } else {
            None
        }
    };
    loop {
        let b = r.get(o, 46);
        let sig = u32le(&b, 0);
        match sig {
            0x0403_4b50 => {
                let flags = u16le(&b, 6);
                let csize = u32le(&b, 18) as u64;
                let nl = u16le(&b, 26) as u64;
                let xl = u16le(&b, 28) as u64;
                let name = r.get(o + 30, nl.min(256) as usize);
                if count < 64 && ext == "zip" {
                    if let Some(x) = name_rule(&name) {
                        ext = x;
                    }
                    if name == b"mimetype" {
                        let m = r.get(o + 30 + nl + xl, 64);
                        let t = String::from_utf8_lossy(&m);
                        if t.contains("opendocument.text") {
                            ext = "odt";
                        } else if t.contains("opendocument.spreadsheet") {
                            ext = "ods";
                        } else if t.contains("opendocument.presentation") {
                            ext = "odp";
                        } else if t.contains("epub") {
                            ext = "epub";
                        }
                    }
                }
                count += 1;
                if flags & 8 != 0 && csize == 0 {
                    // The size is in the data descriptor: look for the next header.
                    let mut next = None;
                    r.scan(o + 30 + nl + xl, 4 * GB, |d, off| {
                        for (i, w) in d.windows(4).enumerate() {
                            if w == b"PK\x03\x04" || w == b"PK\x01\x02" {
                                next = Some(off + i as u64);
                                return true;
                            }
                        }
                        false
                    });
                    match next {
                        Some(n) => o = n,
                        None => return None,
                    }
                } else {
                    o += 30 + nl + xl + csize;
                }
            }
            0x0201_4b50 => {
                central = true;
                o += 46 + u16le(&b, 28) as u64 + u16le(&b, 30) as u64 + u16le(&b, 32) as u64;
            }
            0x0605_4b50 => {
                return Hit::new(ext, o + 22 + u16le(&b, 20) as u64, true);
            }
            0x0606_4b50 => o += 12 + u64le(&b, 4),
            0x0706_4b50 => o += 20,
            0x0808_4b50 => o += 16,
            _ => {
                if count == 0 {
                    return None;
                }
                return Hit::new(ext, o, central);
            }
        }
        if o > 64 * GB {
            return None;
        }
    }
}

fn rar(h: &[u8], r: &R) -> Option<Hit> {
    if h[6] == 1 {
        let mut o = 8u64;
        for _ in 0..1 << 20 {
            let b = r.get(o, 32);
            let mut p = 4;
            let (hs, n, _) = rvint(&b[p..])?;
            p += n;
            let (ty, n, _) = rvint(&b[p..])?;
            p += n;
            let (fl, n, _) = rvint(&b[p..])?;
            p += n;
            let mut data = 0;
            if fl & 1 != 0 {
                let (_, n, _) = rvint(&b[p..])?;
                p += n;
            }
            if fl & 2 != 0 {
                data = rvint(&b[p..])?.0;
            }
            o += 4 + vlen(&b[4..]) as u64 + hs + data;
            if ty == 5 {
                return Hit::new("rar", o, true);
            }
            if ty > 5 || ty == 0 {
                return Hit::new("rar", o, false);
            }
        }
        None
    } else {
        let mut o = 7u64;
        for _ in 0..1 << 20 {
            let b = r.get(o, 32);
            let ty = b[2];
            let fl = u16le(&b, 3);
            let sz = u16le(&b, 5) as u64;
            if !(0x72..=0x7B).contains(&ty) || sz < 7 {
                return Hit::new("rar", o, false);
            }
            let add = if fl & 0x8000 != 0 { u32le(&b, 7) as u64 } else { 0 };
            o += sz + add;
            if ty == 0x7B {
                return Hit::new("rar", o, true);
            }
        }
        None
    }
}

fn rvint(b: &[u8]) -> Option<(u64, usize, bool)> {
    let mut v = 0u64;
    for (i, c) in b.iter().enumerate().take(10) {
        v |= ((*c & 0x7F) as u64) << (7 * i);
        if c & 0x80 == 0 {
            return Some((v, i + 1, false));
        }
    }
    None
}

fn vlen(b: &[u8]) -> usize {
    rvint(b).map(|x| x.1).unwrap_or(1)
}

struct Counting<'a, 'b> {
    r: &'a R<'b>,
    pos: u64,
    buf: Vec<u8>,
    bo: usize,
}

impl Read for Counting<'_, '_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let b = self.fill_buf()?;
        let n = b.len().min(out.len());
        out[..n].copy_from_slice(&b[..n]);
        self.consume(n);
        Ok(n)
    }
}

impl BufRead for Counting<'_, '_> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.bo >= self.buf.len() {
            if self.pos >= self.r.limit {
                self.buf.clear();
                self.bo = 0;
                return Ok(&[]);
            }
            let n = (self.r.limit - self.pos).min(1 << 16) as usize;
            self.buf = self.r.get(self.pos, n);
            self.bo = 0;
        }
        Ok(&self.buf[self.bo..])
    }
    fn consume(&mut self, n: usize) {
        self.bo += n;
        self.pos += n as u64;
    }
}

fn gzip(r: &R) -> Option<Hit> {
    let c = Counting { r, pos: 0, buf: Vec::new(), bo: 0 };
    let mut d = flate2::bufread::GzDecoder::new(c);
    let mut sink = vec![0u8; 1 << 16];
    let mut total = 0u64;
    loop {
        match d.read(&mut sink) {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                if total > 32 * GB {
                    return None;
                }
            }
            Err(_) => {
                if total < 1024 {
                    return None;
                }
                let pos = d.into_inner().pos;
                return Hit::new("gz", pos, false);
            }
        }
    }
    let pos = d.into_inner().pos;
    (total > 0).then_some(())?;
    Hit::new("gz", pos, true)
}

fn zstd_frames(r: &R) -> Option<Hit> {
    let mut o = 0u64;
    let mut frames = 0;
    loop {
        let h = r.get(o, 32);
        if u32le(&h, 0) != 0xFD2F_B528 {
            break;
        }
        let d = h[4];
        let fcs = d >> 6;
        let single = d & 0x20 != 0;
        let has_csum = d & 4 != 0;
        let dict = [0u64, 1, 2, 4][(d & 3) as usize];
        let fsize = match fcs {
            0 => u64::from(single),
            1 => 2,
            2 => 4,
            _ => 8,
        };
        o += 5 + u64::from(!single) + dict + fsize;
        loop {
            let b = r.get(o, 3);
            let w = b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32) << 16;
            let last = w & 1 != 0;
            let ty = (w >> 1) & 3;
            let sz = (w >> 3) as u64;
            if ty == 3 {
                return if frames > 0 { Hit::new("zst", o, false) } else { None };
            }
            o += 3 + if ty == 1 { 1 } else { sz };
            if last {
                break;
            }
            if o > 32 * GB {
                return None;
            }
        }
        if has_csum {
            o += 4;
        }
        frames += 1;
    }
    (frames > 0).then_some(())?;
    Hit::new("zst", o, true)
}

fn elf(h: &[u8]) -> Option<Hit> {
    let bits64 = h[4] == 2;
    let le = h[5] == 1;
    if !(h[4] == 1 || h[4] == 2) || !(h[5] == 1 || h[5] == 2) || h[6] != 1 {
        return None;
    }
    let r16 = |o| if le { u16le(h, o) } else { u16be(h, o) } as u64;
    let r32 = |o| if le { u32le(h, o) } else { u32be(h, o) } as u64;
    let r64 = |o| if le { u64le(h, o) } else { u64be(h, o) };
    let (shoff, ent, num) =
        if bits64 { (r64(40), r16(58), r16(60)) } else { (r32(32), r16(46), r16(48)) };
    let ty = r16(16);
    if !(1..=4).contains(&ty) || ent == 0 || ent > 128 || num == 0 || shoff == 0 {
        return None;
    }
    Hit::new(if ty == 3 { "so" } else { "elf" }, shoff + ent * num, true)
}

fn pe(h: &[u8], r: &R) -> Option<Hit> {
    let lfa = u32le(h, 0x3C) as u64;
    if !(0x40..=4096).contains(&lfa) {
        return None;
    }
    let p = r.get(lfa, 512);
    if &p[..4] != b"PE\0\0" {
        return None;
    }
    let n = u16le(&p, 6) as usize;
    let opt = u16le(&p, 20) as usize;
    let chars = u16le(&p, 22);
    if n == 0 || n > 96 {
        return None;
    }
    let magic = u16le(&p, 24);
    let dd = if magic == 0x20B { 24 + 112 } else { 24 + 96 };
    let cert_off = u32le(&p, dd + 32) as u64;
    let cert_len = u32le(&p, dd + 36) as u64;
    let s = r.get(lfa + 24 + opt as u64, n * 40);
    let mut end = 0u64;
    for i in 0..n {
        let e = &s[i * 40..i * 40 + 40];
        end = end.max(u32le(e, 20) as u64 + u32le(e, 16) as u64);
    }
    end = end.max(cert_off + cert_len);
    if end < 1024 {
        return None;
    }
    Hit::new(if chars & 0x2000 != 0 { "dll" } else { "exe" }, end, true)
}

fn sqlite(h: &[u8]) -> Option<Hit> {
    let ps = match u16be(h, 16) {
        1 => 65536,
        p if p.is_power_of_two() && p >= 512 => p as u64,
        _ => return None,
    };
    let pages = u32be(h, 28) as u64;
    if pages == 0 {
        return None;
    }
    Hit::new("sqlite", ps * pages, true)
}

fn ole(h: &[u8], r: &R) -> Option<Hit> {
    let shift = u16le(h, 30) as u32;
    if shift != 9 && shift != 12 {
        return None;
    }
    let ss = 1u64 << shift;
    let nfat = u32le(h, 44) as usize;
    if nfat == 0 || nfat > 1 << 20 {
        return None;
    }
    let mut last = 0u64;
    let per = (ss / 4) as usize;
    for i in 0..nfat.min(109) {
        let sec = u32le(h, 76 + i * 4);
        if sec >= 0xFFFF_FFFA {
            continue;
        }
        let b = r.get((sec as u64 + 1) * ss, ss as usize);
        for k in 0..per {
            if u32le(&b, k * 4) != 0xFFFF_FFFF {
                last = last.max(i as u64 * per as u64 + k as u64);
            }
        }
    }
    let size = (last + 2) * ss;
    let dir = r.get((u32le(h, 48) as u64 + 1) * ss, 4096.min(ss as usize * 4));
    let names: Vec<u16> = dir.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let text = String::from_utf16_lossy(&names);
    let ext = if text.contains("WordDocument") {
        "doc"
    } else if text.contains("Workbook") || text.contains("Book") {
        "xls"
    } else if text.contains("PowerPoint") {
        "ppt"
    } else {
        "msi"
    };
    Hit::new(ext, size, true)
}

fn psd(h: &[u8], r: &R) -> Option<Hit> {
    let ch = u16be(h, 12) as u64;
    let hgt = u32be(h, 14) as u64;
    let wid = u32be(h, 18) as u64;
    let depth = u16be(h, 22) as u64;
    if ch == 0 || ch > 56 || hgt == 0 || wid == 0 || depth == 0 {
        return None;
    }
    let mut o = 26u64;
    for _ in 0..3 {
        o += 4 + u32be(&r.get(o, 4), 0) as u64;
    }
    let comp = u16be(&r.get(o, 2), 0);
    o += 2;
    let sz = match comp {
        0 => wid * hgt * ch * depth / 8,
        1 => {
            let n = hgt * ch;
            let t = r.get(o, (n as usize * 2).min(64 << 20));
            n * 2 + t.chunks_exact(2).map(|c| u16be(c, 0) as u64).sum::<u64>()
        }
        _ => return Hit::new("psd", r.until_gap(o, 2 * GB), false),
    };
    Hit::new("psd", o + sz, true)
}

fn flv(h: &[u8], r: &R) -> Option<Hit> {
    let mut o = u32be(h, 5) as u64 + 4;
    let mut tags = 0;
    loop {
        let b = r.get(o, 11);
        if !matches!(b[0], 8 | 9 | 18) {
            break;
        }
        let n = ((b[1] as u64) << 16) | ((b[2] as u64) << 8) | b[3] as u64;
        o += 15 + n;
        tags += 1;
        if o > 32 * GB {
            break;
        }
    }
    (tags > 2).then_some(())?;
    Hit::new("flv", o, true)
}

fn asf(h: &[u8], r: &R) -> Option<Hit> {
    const FILE_PROPS: [u8; 16] =
        [0xA1, 0xDC, 0xAB, 0x8C, 0x47, 0xA9, 0xCF, 0x11, 0x8E, 0xE4, 0x00, 0xC0, 0x0C, 0x20, 0x53, 0x65];
    let hs = u64le(h, 16).min(1 << 16);
    let b = r.get(0, hs as usize);
    let i = b.windows(16).position(|w| w == FILE_PROPS)?;
    let size = u64le(&b, i + 40);
    let ext = if b.windows(16).any(|w| w[..4] == [0xC0, 0xEF, 0x19, 0xBC]) { "wmv" } else { "wma" };
    Hit::new(ext, size, true)
}

fn midi(r: &R) -> Option<Hit> {
    let mut o = 14u64;
    let b = r.get(0, 14);
    if u32be(&b, 4) != 6 {
        return None;
    }
    let tracks = u16be(&b, 10);
    for _ in 0..tracks {
        let t = r.get(o, 8);
        if &t[..4] != b"MTrk" {
            return Hit::new("mid", o, false);
        }
        o += 8 + u32be(&t, 4) as u64;
    }
    Hit::new("mid", o, true)
}

fn font(h: &[u8], r: &R) -> Option<Hit> {
    let n = u16be(h, 4) as usize;
    if n == 0 || n > 64 {
        return None;
    }
    let b = r.get(12, n * 16);
    let mut end = 0u64;
    for i in 0..n {
        let e = &b[i * 16..i * 16 + 16];
        if !e[..4].iter().all(|c| c.is_ascii_graphic() || *c == b' ') {
            return None;
        }
        end = end.max(u32be(e, 8) as u64 + u32be(e, 12) as u64);
    }
    if end < 512 || end > GB {
        return None;
    }
    Hit::new(if &h[..4] == b"OTTO" { "otf" } else { "ttf" }, end, true)
}

fn mpegts(r: &R) -> Option<Hit> {
    let mut o = 0u64;
    let mut buf = Vec::new();
    let mut bo = 0u64;
    loop {
        if o < bo || o >= bo + buf.len() as u64 {
            bo = o;
            buf = r.get(o, 188 * 8192);
        }
        if buf[(o - bo) as usize] != 0x47 {
            break;
        }
        o += 188;
        if o >= r.limit {
            break;
        }
    }
    if o < 188 * 64 {
        return None;
    }
    Hit::new("ts", o, false)
}

fn ar(r: &R) -> Option<Hit> {
    let mut o = 8u64;
    let mut ext = "a";
    let mut n = 0;
    loop {
        let h = r.get(o, 60);
        if &h[58..60] != b"`\n" {
            break;
        }
        if n == 0 && h.starts_with(b"debian-binary") {
            ext = "deb";
        }
        let s = String::from_utf8_lossy(&h[48..58]).trim().to_string();
        let sz: u64 = s.parse().ok()?;
        o += 60 + sz + (sz & 1);
        n += 1;
        if o > 64 * GB {
            return None;
        }
    }
    (n > 0).then_some(())?;
    Hit::new(ext, o, true)
}

/// Tells a file's real type from its first bytes; used in verification and for
/// records with an unknown name.
pub fn sniff(h: &[u8]) -> Option<&'static str> {
    if h.len() < 16 {
        return None;
    }
    if &h[4..8] == b"ftyp" {
        return Some(match &h[8..12] {
            b"heic" | b"heix" | b"mif1" | b"msf1" => "heic",
            b"avif" => "avif",
            b"M4A " => "m4a",
            b"qt  " => "mov",
            _ => "mp4",
        });
    }
    Some(match h {
        [0xFF, 0xD8, 0xFF, ..] => "jpg",
        [0x89, b'P', b'N', b'G', ..] => "png",
        [b'G', b'I', b'F', b'8', ..] => "gif",
        [b'%', b'P', b'D', b'F', ..] => "pdf",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "webp",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'A', b'V', b'E', ..] => "wav",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'A', b'V', b'I', b' ', ..] => "avi",
        [b'P', b'K', 3, 4, ..] => "zip",
        [0x1A, 0x45, 0xDF, 0xA3, ..] => "mkv",
        [b'I', b'D', b'3', ..] => "mp3",
        [b'f', b'L', b'a', b'C', ..] => "flac",
        [b'O', b'g', b'g', b'S', ..] => "ogg",
        [b'R', b'a', b'r', b'!', ..] => "rar",
        [0x37, 0x7A, 0xBC, 0xAF, ..] => "7z",
        [0x1F, 0x8B, 8, ..] => "gz",
        [0xFD, b'7', b'z', b'X', b'Z', 0, ..] => "xz",
        [0x28, 0xB5, 0x2F, 0xFD, ..] => "zst",
        [0x7F, b'E', b'L', b'F', ..] => "elf",
        [b'S', b'Q', b'L', b'i', b't', b'e', ..] => "sqlite",
        [0xD0, 0xCF, 0x11, 0xE0, ..] => "doc",
        [b'8', b'B', b'P', b'S', ..] => "psd",
        [b'M', b'Z', ..] => "exe",
        [b'B', b'M', ..] => "bmp",
        [b'I', b'I', 0x2A, 0, ..] | [b'M', b'M', 0, 0x2A, ..] => "tif",
        _ => return None,
    })
}

/// The signature family expected on disk for an extension.
pub fn family(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "jpg" | "jpeg" | "jfif" => "jpg",
        "png" => "png",
        "gif" => "gif",
        "pdf" => "pdf",
        "webp" => "webp",
        "wav" => "wav",
        "avi" => "avi",
        "zip" | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" | "epub" | "apk" | "jar" => "zip",
        "mkv" | "webm" => "mkv",
        "flac" => "flac",
        "ogg" | "opus" | "oga" | "ogv" => "ogg",
        "rar" => "rar",
        "7z" => "7z",
        "gz" | "tgz" => "gz",
        "xz" => "xz",
        "zst" => "zst",
        "sqlite" | "sqlite3" => "sqlite",
        "doc" | "xls" | "ppt" | "msi" => "doc",
        "psd" => "psd",
        "exe" | "dll" => "exe",
        "bmp" => "bmp",
        "tif" | "tiff" => "tif",
        "mp4" | "m4v" | "mov" | "m4a" | "heic" | "heif" | "avif" | "3gp" => "bmff",
        _ => return None,
    })
}

pub fn family_of_sniff(s: &str) -> &str {
    match s {
        "mp4" | "mov" | "m4a" | "heic" | "avif" => "bmff",
        o => o,
    }
}

pub fn kind_of(ext: &str) -> Kind {
    Kind::from_ext(ext)
}
