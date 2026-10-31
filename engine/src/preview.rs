//! Previews: thumbnails and temporary extraction. All output goes to the user's own
//! cache, because the engine has already dropped root at this point.

use crate::model::*;
use crate::reader;
use crate::source::Source;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const MAX_IMAGE_READ: u64 = 96 << 20;
const MAX_MEDIA_READ: u64 = 64 << 20;
const MAX_EXTRACT: u64 = 1 << 30;

/// Preview output is kept in RAM. The scanned disk may be the user's own disk:
/// writing to ~/.cache would mean writing over the data to be recovered.
pub fn cache_dir(session: &str) -> PathBuf {
    let uid = unsafe { libc::geteuid() };
    let run = PathBuf::from(format!("/run/user/{uid}"));
    let base = if run.is_dir() { run } else { std::env::temp_dir() };
    let d = base.join("salvage").join(session);
    let _ = fs::create_dir_all(&d);
    d
}

/// Clears old caches left by crashed sessions.
pub fn sweep_stale() {
    let uid = unsafe { libc::geteuid() };
    let Ok(rd) = fs::read_dir(format!("/run/user/{uid}/salvage")) else { return };
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
        if old.map(|d| d.as_secs() > 86400).unwrap_or(false) {
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

fn run_quiet(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Builds a thumbnail of a record and returns the file path.
pub fn thumb(src: &Source, e: &Entry, size: u32, session: &str) -> Option<PathBuf> {
    let dir = cache_dir(session);
    let out = dir.join(format!("t{}_{size}.jpg", e.id));
    if out.exists() {
        return Some(out);
    }
    match e.kind {
        Kind::Image => {
            let limit = e.size.min(MAX_IMAGE_READ) as usize;
            let bytes = reader::head(src, e, limit);
            if let Ok(img) = image::load_from_memory(&bytes) {
                let t = img.thumbnail(size, size).to_rgb8();
                let mut c = Cursor::new(Vec::new());
                t.write_to(&mut c, image::ImageFormat::Jpeg).ok()?;
                fs::write(&out, c.into_inner()).ok()?;
                return Some(out);
            }
            // Try ffmpeg for heic, raw and others.
            let tmp = dir.join(format!("x{}.{}", e.id, safe_ext(e)));
            fs::write(&tmp, &bytes).ok()?;
            let ok = frame(&tmp, &out, size);
            let _ = fs::remove_file(&tmp);
            ok.then_some(out)
        }
        Kind::Video => {
            let tmp = dir.join(format!("x{}.{}", e.id, safe_ext(e)));
            reader::to_file(src, e, &tmp, MAX_MEDIA_READ).ok()?;
            let ok = frame(&tmp, &out, size);
            let _ = fs::remove_file(&tmp);
            ok.then_some(out)
        }
        Kind::Document if e.ext == "pdf" => {
            let tmp = dir.join(format!("x{}.pdf", e.id));
            reader::to_file(src, e, &tmp, MAX_MEDIA_READ).ok()?;
            let stem = dir.join(format!("p{}", e.id));
            let ok = run_quiet(
                "pdftoppm",
                &["-jpeg", "-singlefile", "-f", "1", "-l", "1", "-scale-to", &size.to_string(), s(&tmp)?, s(&stem)?],
            );
            let _ = fs::remove_file(&tmp);
            let made = stem.with_extension("jpg");
            if ok && made.exists() {
                fs::rename(made, &out).ok()?;
                Some(out)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn s(p: &Path) -> Option<&str> {
    p.to_str()
}

fn safe_ext(e: &Entry) -> String {
    let x: String = e.ext.chars().filter(char::is_ascii_alphanumeric).take(8).collect();
    if x.is_empty() {
        "bin".into()
    } else {
        x
    }
}

fn frame(input: &Path, out: &Path, size: u32) -> bool {
    let (Some(i), Some(o)) = (input.to_str(), out.to_str()) else { return false };
    if run_quiet("ffmpegthumbnailer", &["-i", i, "-o", o, "-s", &size.to_string(), "-q", "6"]) && out.exists() {
        return true;
    }
    let scale = format!("scale={size}:-2");
    run_quiet("ffmpeg", &["-v", "quiet", "-y", "-i", i, "-frames:v", "1", "-vf", &scale, o]) && out.exists()
}

/// Extracts a file to the cache for preview; the system's default app opens it.
pub fn extract(src: &Source, e: &Entry, session: &str) -> Option<PathBuf> {
    // A file too big for RAM is not previewed; it has to be recovered to open.
    if e.size > MAX_EXTRACT {
        return None;
    }
    let dir = cache_dir(session).join("open");
    fs::create_dir_all(&dir).ok()?;
    let name = crate::recover::sanitize(&e.name);
    let path = dir.join(format!("{}_{name}", e.id));
    if !path.exists() {
        reader::to_file(src, e, &path, u64::MAX).ok()?;
    }
    Some(path)
}
