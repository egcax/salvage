//! Data model of found files and a queryable catalog.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// The file system record is marked as deleted.
    Deleted,
    /// In a trash or recycle bin.
    Trash,
    /// No file system record, found by signature search.
    Carved,
    /// Orphan record whose parent folder is unknown.
    Orphan,
    /// In a snapshot, missing from the live file system.
    Snapshot,
    /// Still present.
    Existing,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Image,
    Video,
    Audio,
    Document,
    Archive,
    Code,
    App,
    Database,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 9] = [
        Kind::Image,
        Kind::Video,
        Kind::Audio,
        Kind::Document,
        Kind::Archive,
        Kind::Code,
        Kind::App,
        Kind::Database,
        Kind::Other,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Kind::Image => "image",
            Kind::Video => "video",
            Kind::Audio => "audio",
            Kind::Document => "document",
            Kind::Archive => "archive",
            Kind::Code => "code",
            Kind::App => "app",
            Kind::Database => "database",
            Kind::Other => "other",
        }
    }
    pub fn from_ext(ext: &str) -> Kind {
        match ext {
            "jpg" | "jpeg" | "jfif" | "png" | "gif" | "bmp" | "tif" | "tiff" | "webp" | "heic"
            | "heif" | "avif" | "svg" | "ico" | "psd" | "raw" | "cr2" | "cr3" | "nef" | "arw"
            | "dng" | "orf" | "rw2" | "raf" | "xcf" | "exr" | "tga" | "jxl" => Kind::Image,
            "mp4" | "m4v" | "mov" | "mkv" | "webm" | "avi" | "wmv" | "flv" | "mpg" | "mpeg"
            | "3gp" | "3g2" | "ts" | "mts" | "m2ts" | "vob" | "ogv" | "asf" => Kind::Video,
            "mp3" | "wav" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "aac" | "wma" | "aiff"
            | "aif" | "mid" | "midi" | "amr" | "ape" => Kind::Audio,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp"
            | "rtf" | "txt" | "md" | "epub" | "mobi" | "csv" | "tex" | "pages" | "numbers"
            | "key" | "djvu" | "vcf" | "ics" => Kind::Document,
            "zip" | "rar" | "7z" | "gz" | "tgz" | "bz2" | "xz" | "zst" | "tar" | "iso" | "cab"
            | "lz4" | "lzma" | "img" | "vhd" | "vhdx" | "vmdk" | "qcow2" | "vdi" => Kind::Archive,
            "c" | "h" | "cpp" | "hpp" | "cc" | "rs" | "py" | "js" | "ts_" | "jsx" | "tsx" | "java"
            | "kt" | "go" | "rb" | "php" | "sh" | "bash" | "fish" | "lua" | "dart" | "cs"
            | "swift" | "html" | "htm" | "css" | "scss" | "xml" | "json" | "yaml" | "yml"
            | "toml" | "ini" | "conf" | "sql" | "qml" | "vue" | "pem" => Kind::Code,
            "exe" | "dll" | "msi" | "elf" | "so" | "apk" | "jar" | "deb" | "rpm" | "appimage"
            | "sys" | "dmg" => Kind::App,
            "sqlite" | "sqlite3" | "db" | "mdb" | "accdb" | "kdbx" => Kind::Database,
            _ => Kind::Other,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum Piece {
    /// Raw bytes from the source (absolute offset).
    Data { off: u64, len: u64 },
    /// Sparse region.
    Zero { len: u64 },
    /// Data stored inside metadata.
    Inline { data: Vec<u8> },
    /// Compressed btrfs extent. `skip`/`len` refer to the decompressed data.
    Comp { off: u64, clen: u64, codec: u8, ulen: u64, skip: u64, len: u64 },
    /// NTFS LZNT1 compression unit.
    Lznt { off: u64, clen: u64, ulen: u64 },
    /// File on a mounted file system (trash).
    Host { path: String, len: u64 },
}

impl Piece {
    pub fn out_len(&self) -> u64 {
        match self {
            Piece::Data { len, .. } => *len,
            Piece::Zero { len } => *len,
            Piece::Inline { data } => data.len() as u64,
            Piece::Comp { len, .. } => *len,
            Piece::Lznt { ulen, .. } => *ulen,
            Piece::Host { len, .. } => *len,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Entry {
    pub id: u32,
    pub vol: u16,
    pub dir: u32,
    pub name: String,
    pub ext: String,
    pub size: u64,
    pub state: State,
    pub kind: Kind,
    pub method: String,
    pub mtime: i64,
    pub ctime: i64,
    pub dtime: i64,
    pub chance: u8,
    pub note: Option<String>,
    pub pieces: Vec<Piece>,
    /// The chance was measured by checksum; estimated verification is skipped.
    #[serde(default)]
    pub verified: bool,
    /// A better copy with the same content exists.
    #[serde(default)]
    pub dup: bool,
}

impl Entry {
    pub fn new(vol: u16, dir: u32, name: String, size: u64, state: State, method: &str) -> Entry {
        let ext = ext_of(&name);
        let kind = Kind::from_ext(&ext);
        Entry {
            id: 0,
            vol,
            dir,
            name,
            ext,
            size,
            state,
            kind,
            method: method.to_string(),
            mtime: 0,
            ctime: 0,
            dtime: 0,
            chance: 90,
            note: None,
            pieces: Vec::new(),
            verified: false,
            dup: false,
        }
    }
    pub fn first_off(&self) -> Option<u64> {
        self.pieces.iter().find_map(|p| match p {
            Piece::Data { off, .. } => Some(*off),
            Piece::Comp { off, .. } => Some(*off),
            Piece::Lznt { off, .. } => Some(*off),
            _ => None,
        })
    }
}

pub fn ext_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() && name.len() - i <= 12 => name[i + 1..].to_lowercase(),
        _ => String::new(),
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Dir {
    pub parent: u32,
    pub name: String,
    pub vol: u16,
    #[serde(default)]
    pub deleted: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct VolumeInfo {
    pub index: u16,
    pub fstype: String,
    pub label: String,
    pub offset: u64,
    pub size: u64,
    pub block_size: u32,
    pub root: u32,
    pub lost: bool,
    pub free_bytes: u64,
    pub zero_ratio: Option<f64>,
    pub note: Option<String>,
}

pub const NO_PARENT: u32 = u32::MAX;

#[derive(Serialize, Deserialize, Default)]
pub struct Catalog {
    pub entries: Vec<Entry>,
    pub dirs: Vec<Dir>,
    pub volumes: Vec<VolumeInfo>,
    #[serde(skip)]
    pub version: u64,
    /// Remaining regions of an interrupted signature scan.
    #[serde(default)]
    pub pending: Vec<(u64, u64)>,
    /// Results of the last content search.
    #[serde(skip)]
    pub grep: Option<HashSet<u32>>,
    #[serde(skip)]
    dir_index: HashMap<(u32, String), u32>,
    #[serde(skip)]
    cache: Option<(String, u64, Vec<u32>)>,
}

#[derive(Deserialize, Default, Clone, Serialize, Debug)]
#[serde(default)]
pub struct Filter {
    pub states: Vec<State>,
    pub kinds: Vec<Kind>,
    pub text: String,
    pub exts: Vec<String>,
    pub dir: Option<u32>,
    pub recursive: bool,
    pub vol: Option<u16>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub min_chance: Option<u8>,
    pub hide_dupes: bool,
    pub grep_only: bool,
}

#[derive(Deserialize, Default, Clone, Serialize, Debug)]
#[serde(default)]
pub struct Sort {
    pub by: String,
    pub desc: bool,
}

#[derive(Serialize, Default)]
pub struct Facets {
    /// High, medium, low, lost: (count, bytes).
    pub chances: [(u64, u64); 4],
    pub kinds: HashMap<String, (u64, u64)>,
    pub states: HashMap<String, (u64, u64)>,
    pub total: u64,
    pub bytes: u64,
}

impl Catalog {
    pub fn rebuild_index(&mut self) {
        self.dir_index.clear();
        for (i, d) in self.dirs.iter().enumerate() {
            self.dir_index.insert((d.parent, d.name.clone()), i as u32);
        }
    }

    pub fn add_root(&mut self, vol: u16, name: &str) -> u32 {
        let id = self.dirs.len() as u32;
        self.dirs.push(Dir { parent: NO_PARENT, name: name.to_string(), vol, deleted: false });
        id
    }

    pub fn child_dir(&mut self, parent: u32, name: &str, deleted: bool) -> u32 {
        if let Some(&id) = self.dir_index.get(&(parent, name.to_string())) {
            return id;
        }
        let id = self.dirs.len() as u32;
        let vol = self.dirs.get(parent as usize).map(|d| d.vol).unwrap_or(0);
        self.dirs.push(Dir { parent, name: name.to_string(), vol, deleted });
        self.dir_index.insert((parent, name.to_string()), id);
        id
    }

    pub fn add(&mut self, mut e: Entry) -> u32 {
        e.id = self.entries.len() as u32;
        let id = e.id;
        self.entries.push(e);
        self.version += 1;
        id
    }

    pub fn path_of(&self, dir: u32) -> String {
        let mut parts = Vec::new();
        let mut cur = dir;
        let mut guard = 0;
        while cur != NO_PARENT && guard < 512 {
            match self.dirs.get(cur as usize) {
                Some(d) => {
                    if d.parent != NO_PARENT {
                        parts.push(d.name.as_str());
                    }
                    cur = d.parent;
                }
                None => break,
            }
            guard += 1;
        }
        parts.reverse();
        parts.join("/")
    }

    /// Is the folder inside a trash folder?
    pub fn in_trash(&self, dir: u32) -> bool {
        let mut cur = dir;
        let mut guard = 0;
        while cur != NO_PARENT && guard < 256 {
            let Some(d) = self.dirs.get(cur as usize) else { return false };
            if d.name.starts_with(".Trash-") || d.name == ".Trash" || d.name.eq_ignore_ascii_case("$RECYCLE.BIN") {
                return true;
            }
            cur = d.parent;
            guard += 1;
        }
        false
    }

    fn is_under(&self, dir: u32, root: u32) -> bool {
        let mut cur = dir;
        let mut guard = 0;
        while cur != NO_PARENT && guard < 512 {
            if cur == root {
                return true;
            }
            cur = self.dirs.get(cur as usize).map(|d| d.parent).unwrap_or(NO_PARENT);
            guard += 1;
        }
        false
    }

    pub fn matches(&self, e: &Entry, f: &Filter, needle: &str) -> bool {
        if f.hide_dupes && e.dup {
            return false;
        }
        if f.grep_only && !self.grep.as_ref().is_some_and(|g| g.contains(&e.id)) {
            return false;
        }
        if !f.states.is_empty() && !f.states.contains(&e.state) {
            return false;
        }
        if !f.kinds.is_empty() && !f.kinds.contains(&e.kind) {
            return false;
        }
        if let Some(v) = f.vol {
            if e.vol != v {
                return false;
            }
        }
        if let Some(m) = f.min_size {
            if e.size < m {
                return false;
            }
        }
        if let Some(m) = f.max_size {
            if e.size > m {
                return false;
            }
        }
        if let Some(c) = f.min_chance {
            if e.chance < c {
                return false;
            }
        }
        if f.from.is_some() || f.to.is_some() {
            let t = e.mtime.max(e.dtime);
            if let Some(a) = f.from {
                if t < a {
                    return false;
                }
            }
            if let Some(b) = f.to {
                if t > b {
                    return false;
                }
            }
        }
        if !f.exts.is_empty() && !f.exts.iter().any(|x| x == &e.ext) {
            return false;
        }
        if let Some(d) = f.dir {
            let ok = if f.recursive { self.is_under(e.dir, d) } else { e.dir == d };
            if !ok {
                return false;
            }
        }
        if !needle.is_empty() {
            let hay = e.name.to_lowercase();
            if !hay.contains(needle) {
                let p = self.path_of(e.dir).to_lowercase();
                if !p.contains(needle) {
                    return false;
                }
            }
        }
        true
    }

    /// Computes and caches the filter and sort result; paging stays O(1).
    pub fn select(&mut self, f: &Filter, s: &Sort) -> Vec<u32> {
        let key = serde_json::to_string(&(f, s)).unwrap_or_default();
        if let Some((k, v, ids)) = &self.cache {
            if *k == key && *v == self.version {
                return ids.clone();
            }
        }
        let needle = fold_case(&f.text);
        let mut ids: Vec<u32> = self
            .entries
            .iter()
            .filter(|e| self.matches(e, f, &needle))
            .map(|e| e.id)
            .collect();
        let en = &self.entries;
        let cmp = |a: &u32, b: &u32| -> std::cmp::Ordering {
            let (x, y) = (&en[*a as usize], &en[*b as usize]);
            match s.by.as_str() {
                "size" => x.size.cmp(&y.size),
                "date" => x.mtime.max(x.dtime).cmp(&y.mtime.max(y.dtime)),
                "chance" => x.chance.cmp(&y.chance),
                "kind" => x.ext.cmp(&y.ext),
                "name" => x.name.to_lowercase().cmp(&y.name.to_lowercase()),
                _ => x.chance.cmp(&y.chance).then(x.mtime.max(x.dtime).cmp(&y.mtime.max(y.dtime))),
            }
        };
        if s.desc {
            ids.sort_by(|a, b| cmp(b, a));
        } else {
            ids.sort_by(cmp);
        }
        self.cache = Some((key, self.version, ids.clone()));
        ids
    }

    /// Counts on the filter chips: each dimension is counted without its own filter,
    /// so the number of files in an unselected type is still visible.
    pub fn facet_counts(&self, f: &Filter) -> Facets {
        let needle = fold_case(&f.text);
        let (mut no_kind, mut no_state) = (f.clone(), f.clone());
        no_kind.kinds.clear();
        no_state.states.clear();
        let mut fx = Facets::default();
        for k in Kind::ALL {
            fx.kinds.insert(k.name().to_string(), (0, 0));
        }
        for e in &self.entries {
            if self.matches(e, f, &needle) {
                let t = match e.chance {
                    80..=100 => 0,
                    40..=79 => 1,
                    10..=39 => 2,
                    _ => 3,
                };
                fx.chances[t].0 += 1;
                fx.chances[t].1 += e.size;
            }
            if self.matches(e, &no_kind, &needle) {
                let c = fx.kinds.entry(e.kind.name().to_string()).or_insert((0, 0));
                c.0 += 1;
                c.1 += e.size;
            }
            if self.matches(e, &no_state, &needle) {
                let st = serde_json::to_value(e.state).ok().and_then(|v| v.as_str().map(String::from));
                let c = fx.states.entry(st.unwrap_or_default()).or_insert((0, 0));
                c.0 += 1;
                c.1 += e.size;
            }
        }
        fx
    }

    pub fn facets(&self, ids: &[u32]) -> Facets {
        let mut fx = Facets::default();
        for k in Kind::ALL {
            fx.kinds.insert(k.name().to_string(), (0, 0));
        }
        for &i in ids {
            let e = &self.entries[i as usize];
            let c = fx.kinds.entry(e.kind.name().to_string()).or_insert((0, 0));
            c.0 += 1;
            c.1 += e.size;
            let st = serde_json::to_value(e.state).ok().and_then(|v| v.as_str().map(String::from));
            let c = fx.states.entry(st.unwrap_or_default()).or_insert((0, 0));
            c.0 += 1;
            c.1 += e.size;
            fx.total += 1;
            fx.bytes += e.size;
        }
        fx
    }
}

/// Lowercases for search; every dotted and dotless form of I folds to plain i.
pub fn fold_case(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'I' | '\u{130}' | '\u{131}' => 'i',
            c => c.to_lowercase().next().unwrap_or(c),
        })
        .collect()
}

/// A set of byte ranges; holds the used space of a file system.
#[derive(Default, Clone)]
pub struct Intervals {
    items: Vec<(u64, u64)>,
    sorted: bool,
}

impl Intervals {
    pub fn push(&mut self, start: u64, end: u64) {
        if end <= start {
            return;
        }
        if let Some(last) = self.items.last_mut() {
            if last.1 == start {
                last.1 = end;
                return;
            }
        }
        self.items.push((start, end));
        self.sorted = false;
    }
    pub fn normalize(&mut self) {
        self.items.sort_unstable();
        let mut out: Vec<(u64, u64)> = Vec::with_capacity(self.items.len());
        for &(s, e) in &self.items {
            match out.last_mut() {
                Some(l) if s <= l.1 => l.1 = l.1.max(e),
                _ => out.push((s, e)),
            }
        }
        self.items = out;
        self.sorted = true;
    }
    pub fn items(&self) -> &[(u64, u64)] {
        &self.items
    }
    /// Returns how much of a range is used (must be normalized).
    pub fn overlap(&self, start: u64, end: u64) -> u64 {
        let i = self.items.partition_point(|x| x.1 <= start);
        let mut total = 0;
        for &(s, e) in &self.items[i..] {
            if s >= end {
                break;
            }
            total += e.min(end) - s.max(start);
        }
        total
    }
    /// Free ranges inside [start,end).
    pub fn gaps(&self, start: u64, end: u64) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let mut pos = start;
        let i = self.items.partition_point(|x| x.1 <= start);
        for &(s, e) in &self.items[i..] {
            if s >= end {
                break;
            }
            if s > pos {
                out.push((pos, s));
            }
            pos = pos.max(e);
        }
        if pos < end {
            out.push((pos, end));
        }
        out
    }
}
