//! Device list and destination safety check. Needs no root.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::Command;

fn read_sys(p: &str) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// discard option of mounted btrfs/ext4: with TRIM, data disappears quickly.
fn mount_opts() -> Vec<(String, String, String)> {
    fs::read_to_string("/proc/self/mounts")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let p: Vec<&str> = l.split_whitespace().collect();
            (p.len() >= 4).then(|| (p[0].to_string(), p[1].to_string(), p[3].to_string()))
        })
        .collect()
}

fn usage(mountpoint: &str) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(mountpoint).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(c.as_ptr(), &mut st) } == 0)
        .then(|| (st.f_blocks as u64 * st.f_frsize as u64, st.f_bavail as u64 * st.f_frsize as u64))
}

fn fstrim_timer() -> bool {
    Path::new("/etc/systemd/system/timers.target.wants/fstrim.timer").exists()
}

pub fn list() -> Value {
    let out = Command::new("lsblk")
        .args([
            "-J", "-b", "-o",
            "NAME,PATH,KNAME,PKNAME,SIZE,TYPE,FSTYPE,FSVER,LABEL,UUID,MOUNTPOINTS,MODEL,VENDOR,SERIAL,TRAN,RM,HOTPLUG,ROTA,RO,PTTYPE,PARTLABEL,DISC-GRAN,DISC-MAX",
        ])
        .output();
    let mut v: Value = match out {
        Ok(o) => serde_json::from_slice(&o.stdout).unwrap_or(json!({"blockdevices": []})),
        Err(e) => return json!({"error": e.to_string(), "devices": []}),
    };
    let mounts = mount_opts();
    let timer = fstrim_timer();
    let mut devices = Vec::new();
    if let Some(list) = v["blockdevices"].as_array_mut() {
        for d in list.iter_mut() {
            let ty = d["type"].as_str().unwrap_or("").to_string();
            let name = d["name"].as_str().unwrap_or("").to_string();
            if ty != "disk" && ty != "loop" && ty != "rom" {
                continue;
            }
            if name.starts_with("zram") || name.starts_with("sr") || d["size"].as_u64().unwrap_or(0) == 0 {
                continue;
            }
            enrich_tree(d, &mounts, timer);
            d["sysmodel"] = json!(read_sys(&format!("/sys/block/{name}/device/model")));
            d["queue_rotational"] = json!(read_sys(&format!("/sys/block/{name}/queue/rotational")));
            devices.push(d.clone());
        }
    }
    json!({"devices": devices, "fstrim_timer": timer})
}

/// LUKS and LVM volumes are nested under partitions.
fn enrich_tree(d: &mut Value, mounts: &[(String, String, String)], timer: bool) {
    enrich(d, mounts, timer);
    if let Some(ch) = d["children"].as_array_mut() {
        for c in ch.iter_mut() {
            enrich_tree(c, mounts, timer);
        }
    }
}

fn enrich(d: &mut Value, mounts: &[(String, String, String)], timer: bool) {
    let path = d["path"].as_str().unwrap_or("").to_string();
    let mut discard_mount = false;
    let mut mp_for_usage: Option<String> = None;
    for (src, mp, opts) in mounts {
        if src == &path {
            mp_for_usage.get_or_insert(mp.clone());
            if opts.split(',').any(|o| o == "discard" || o.starts_with("discard=")) {
                discard_mount = true;
            }
        }
    }
    let gran = d["disc-gran"].as_u64().unwrap_or(0);
    d["trim"] = json!(gran > 0);
    d["discard_mount"] = json!(discard_mount);
    d["fstrim_timer"] = json!(timer);
    if let Some(mp) = mp_for_usage {
        if let Some((total, free)) = usage(&mp) {
            d["fs_total"] = json!(total);
            d["fs_free"] = json!(free);
        }
        d["mounted_at"] = json!(mp);
    }
}

/// Source device of the mount point holding the path.
fn mount_source(path: &str) -> Option<String> {
    let real = fs::canonicalize(path).ok()?;
    let real = real.to_string_lossy().to_string();
    let info = fs::read_to_string("/proc/self/mountinfo").ok()?;
    let mut best: Option<(usize, String)> = None;
    for l in info.lines() {
        let f: Vec<&str> = l.split(' ').collect();
        if f.len() < 10 {
            continue;
        }
        let mp = f[4].replace("\\040", " ");
        let dash = f.iter().position(|x| *x == "-")?;
        let src = f.get(dash + 2)?.to_string();
        let matches = real == mp || mp == "/" || real.starts_with(&format!("{mp}/"));
        if matches && best.as_ref().map(|b| mp.len() >= b.0).unwrap_or(true) {
            best = Some((mp.len(), src));
        }
    }
    best.map(|b| b.1)
}

fn parent_disk(dev: &str) -> Option<String> {
    let name = Path::new(dev).file_name()?.to_string_lossy().to_string();
    let link = fs::canonicalize(format!("/sys/class/block/{name}")).ok()?;
    if link.join("partition").exists() {
        return link.parent()?.file_name().map(|s| s.to_string_lossy().to_string());
    }
    Some(name)
}

pub fn check_dest(source: &str, path: &str) -> Value {
    let free = crate::recover::free_space(path);
    let dest_dev = mount_source(path);
    let mut same_part = false;
    let mut same_disk = false;
    if let Some(d) = &dest_dev {
        same_part = d == source;
        if let (Some(a), Some(b)) = (parent_disk(d), parent_disk(source)) {
            same_disk = a == b;
        }
    }
    // With a disk image source, writing to the same file system does not harm the data.
    if !source.starts_with("/dev/") {
        same_part = false;
        same_disk = false;
    }
    json!({"free": free, "device": dest_dev, "same_partition": same_part, "same_disk": same_disk})
}

pub fn parent_of(path: &str) -> String {
    parent_disk(path).map(|d| format!("/dev/{d}")).unwrap_or_else(|| path.to_string())
}
