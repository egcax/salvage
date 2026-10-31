//! Joins a JPEG split into two fragments (bifragment gap carving). Where the entropy
//! stream hits an invalid marker is the end of the first fragment; the rest is searched
//! for in the valid block run holding the EOI, and each candidate is decoded and scored.

use crate::source::Source;

const WINDOW: u64 = 32 << 20;
const MAX_TRIES: usize = 48;

/// Could this block be JPEG entropy data?
fn entropy_like(b: &[u8]) -> bool {
    let mut high = 0usize;
    let mut prev = 0u8;
    for c in b {
        if prev == 0xFF && !matches!(*c, 0x00 | 0xD0..=0xD9 | 0xFF) {
            return false;
        }
        high += usize::from(*c >= 0x80);
        prev = *c;
    }
    // Flat images have less variety, but high bytes stay plentiful.
    high >= b.len() / 8
}

/// Looks for a block inside a seemingly complete JPEG that cannot be entropy data.
/// Text has no FF byte, so the parser walks past it without noticing.
pub fn foreign_block(src: &Source, base: u64, size: u64, block: u64) -> Option<u64> {
    let block = block.clamp(512, 65536);
    if size < block * 3 {
        return None;
    }
    let data = src.read_vec(base, size as usize);
    let n = data.len() / block as usize;
    for i in 1..n.saturating_sub(1) {
        let s = &data[i * block as usize..(i + 1) * block as usize];
        // Compressed data always has high bytes, even in flat regions.
        if s.iter().filter(|c| **c >= 0x80).count() < s.len() / 64 {
            return Some(i as u64 * block);
        }
    }
    None
}

/// Discontinuity between rows of the decoded image: a wrong join produces noise.
fn seam_score(img: &image::RgbImage) -> f64 {
    let (w, h) = img.dimensions();
    if w < 8 || h < 8 {
        return f64::MAX;
    }
    let raw = img.as_raw();
    let stride = w as usize * 3;
    let mut worst = 0f64;
    let band = 8usize;
    let mut y = band;
    while y + stride.min(1) < h as usize {
        let mut sum = 0u64;
        let row = &raw[y * stride..(y + 1) * stride];
        let up = &raw[(y - 1) * stride..y * stride];
        for x in (0..stride).step_by(3) {
            sum += row[x].abs_diff(up[x]) as u64;
        }
        worst = worst.max(sum as f64 / w as f64);
        y += band;
    }
    worst
}

pub fn jpeg_bifragment(src: &Source, base: u64, bad_at: u64, block: u64) -> Option<Vec<(u64, u64)>> {
    let block = block.clamp(512, 65536);
    if bad_at < block {
        return None;
    }
    let win = WINDOW.min(src.size.saturating_sub(base + bad_at));
    if win < block {
        return None;
    }
    let b0 = bad_at / block * block;
    let data = src.read_vec(base + b0, win as usize);
    let n = data.len() / block as usize;
    let valid: Vec<bool> = (0..n).map(|i| entropy_like(&data[i * block as usize..(i + 1) * block as usize])).collect();
    let eoi_in = |i: usize| -> Option<usize> {
        let s = &data[i * block as usize..(i + 1) * block as usize];
        s.windows(2).position(|w| w == [0xFF, 0xD9])
    };
    let mut tries = 0;
    let mut best: Option<(f64, Vec<(u64, u64)>)> = None;
    // The first fragment ends before the bad block; a few blocks back are checked too.
    for back in 0..3u64 {
        if b0 < back * block + block {
            break;
        }
        let first = b0 - back * block;
        let head = src.read_vec(base, first as usize);
        let mut i = 1usize;
        while i < n && tries < MAX_TRIES {
            let run_start = valid[i] && !valid[i - 1];
            if !run_start {
                i += 1;
                continue;
            }
            // Look for the EOI in the next valid block run.
            let mut j = i;
            let mut end = None;
            while j < n && valid[j] {
                if let Some(p) = eoi_in(j) {
                    end = Some((j, p));
                    break;
                }
                j += 1;
            }
            if end.is_none() && j < n {
                // Padding after the EOI block can look invalid.
                if let Some(p) = eoi_in(j) {
                    end = Some((j, p));
                }
            }
            if let Some((j2, p)) = end {
                tries += 1;
                let tail_end = j2 * block as usize + p + 2;
                let mut all = head.clone();
                all.extend_from_slice(&data[i * block as usize..tail_end]);
                let dec = image::load_from_memory_with_format(&all, image::ImageFormat::Jpeg);
                if let Ok(img) = dec {
                    let score = seam_score(&img.to_rgb8());
                    if best.as_ref().is_none_or(|b| score < b.0) {
                        let second = b0 + i as u64 * block;
                        best = Some((score, vec![(0, first), (second, (tail_end - i * block as usize) as u64)]));
                    }
                }
            }
            i = j.max(i) + 1;
        }
    }
    best.map(|b| b.1)
}
