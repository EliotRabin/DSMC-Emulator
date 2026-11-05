//! YAFFS/POSIX ABI bridge. Assets are read-only; all guest writes stay in RAM.
use crate::unicorn::{Cpu, LR, PC, R0};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
type FsResult<T> = Result<T, i32>;
const MAX_FILE: usize = 32 * 1024 * 1024;
const MAX_OVERLAY: usize = 64 * 1024 * 1024;
#[derive(Clone)]
struct File {
    path: String,
    pos: usize,
    flags: u32,
}

struct Directory {
    names: Vec<String>,
    pos: usize,
}

pub struct Storage {
    root: PathBuf,
    asset_cache: RefCell<BTreeMap<String, Vec<u8>>>,
    path_cache: RefCell<BTreeMap<String, Option<PathBuf>>>,
    files: BTreeMap<String, Vec<u8>>,
    dirs: BTreeSet<String>,
    links: BTreeMap<String, String>,
    deleted: BTreeSet<String>,
    handles: BTreeMap<u32, File>,
    directories: BTreeMap<u32, Directory>,
    free_dirs: Vec<u32>,
    devices: BTreeMap<String, u32>,
    next: u32,
    pub reads: u64,
    pub writes: u64,
    pub calls: u64,
}

pub fn normalize(path: &str) -> FsResult<String> {
    let p = path.replace('\\', "/");
    let mut result = Vec::new();
    for s in p.split('/') {
        if s == ".." {
            return Err(22);
        }
        if s.contains(':') || s.contains('\0') {
            return Err(22);
        }
        if !s.is_empty() && s != "." {
            result.push(s)
        }
    }
    Ok(format!("/{}", result.join("/")))
}

impl Storage {
    pub fn new(c: Cpu, root: &Path) -> Result<Self, String> {
        let root = root.canonicalize().map_err(|e| e.to_string())?;
        if !root.is_dir() {
            return Err("Le dossier des assets est invalide".into());
        }
        let mut s = Self {
            root,
            asset_cache: RefCell::new(BTreeMap::new()),
            path_cache: RefCell::new(BTreeMap::new()),
            files: BTreeMap::new(),
            dirs: [
                "/", "/env", "/log", "/data", "/pdata", "/cal", "/cal2", "/slot", "/image1",
                "/image2", "/rodata",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            links: BTreeMap::new(),
            deleted: BTreeSet::new(),
            handles: BTreeMap::new(),
            directories: BTreeMap::new(),
            free_dirs: Vec::new(),
            devices: BTreeMap::new(),
            next: 0x40203000,
            reads: 0,
            writes: 0,
            calls: 0,
        };
        s.seed(c)?;
        Ok(s)
    }

    pub fn text(c: Cpu, a: u32) -> FsResult<String> {
        let end = if (0x40203000..0x4020f000).contains(&a) {
            0x4020f000
        } else if (0x80000000..0x88000000).contains(&a) {
            0x88000000
        } else {
            return Err(14);
        };
        let b = c.read(a, 1024.min((end - a) as usize)).map_err(|_| 14)?;
        String::from_utf8(b.split(|v| *v == 0).next().unwrap_or_default().to_vec()).map_err(|_| 22)
    }

    fn seed(&mut self, c: Cpu) -> Result<(), String> {
        self.files
            .insert("/env/CAMERA_TYPE".into(), 15u16.to_le_bytes().to_vec());
        self.files.insert("/env/CAMERA_SUB_TYPE".into(), vec![0; 2]);
        self.files.insert("/env/SENSOR_TYPE".into(), vec![6]);
        let mut serial = vec![0; 65];
        serial[..10].copy_from_slice(b"VIRTUAL-PC");
        self.files.insert("/env/SERIAL_NUMBER".into(), serial);
        self.files
            .insert("/env/BOOT_IMAGE".into(), 1u32.to_le_bytes().to_vec());
        let v = c.word(0x81067890 + 15 * 4);
        let t = 0x81067228 + v * 24;
        let count = c.word(t + 4);
        let records = c.word(t + 8);
        let aliases = c.word(t + 16);
        if count > 32 || aliases > 128 {
            return Err("Table de partitions incompatible avec RED 6.3.106".into());
        }
        let mut data = Vec::new();
        for word in [v, count] {
            data.extend(word.to_le_bytes())
        }
        data.extend(c.read(records, count as usize * 24)?);
        for word in [c.word(t + 12), aliases] {
            data.extend(word.to_le_bytes())
        }
        for i in 0..aliases {
            for field in [0, 4] {
                let text = Self::text(c, c.word(c.word(t + 20) + i * 8 + field))
                    .map_err(|e| format!("partition errno {e}"))?;
                data.extend((text.len() as u32).to_le_bytes());
                data.extend(text.as_bytes())
            }
        }
        self.files.insert("/env/PARTITION_TABLE".into(), data);
        let mut digest = md5::Context::new();
        digest.consume(c.read(0x80d44750, 6)?);
        digest.consume([0u8; 16]);
        for i in 0..46 {
            let e = 0x81066b98 + i * 36;
            let path = Self::text(c, c.word(e + 4)).map_err(|_| "invalid environment path")?;
            if c.read(e + 22, 1)?[0] != 0 {
                if let Some(v) = self
                    .files
                    .get(&normalize(&path).map_err(|_| "invalid environment path")?)
                {
                    digest.consume(&v[..v.len().min(c.word(e + 28) as usize)])
                }
            }
        }
        self.files
            .insert("/env/SIGNATURE".into(), digest.compute().0.to_vec());
        Ok(())
    }

    fn resolve(&self, p: &str) -> FsResult<String> {
        let mut p = normalize(p)?;
        for _ in 0..16 {
            let mut found = None;
            for (link, target) in self.links.iter().rev() {
                if p == *link || p.starts_with(&format!("{link}/")) {
                    let t = if target.starts_with('/') {
                        target.clone()
                    } else {
                        format!("{}/{}", parent(link), target)
                    };
                    found = Some(normalize(&format!("{t}{}", &p[link.len()..]))?);
                    break;
                }
            }
            match found {
                Some(v) => p = v,
                None => return Ok(p),
            }
        }
        Err(40)
    }

    fn asset(&self, p: &str) -> Option<PathBuf> {
        if let Some(q) = self.path_cache.borrow().get(p) {
            return q.clone();
        }
        let result = (|| {
            let parts: Vec<_> = p.trim_start_matches('/').split('/').collect();
            if !["slot", "image1", "image2", "rodata"].contains(parts.first()?) {
                return None;
            }
            let mut tail = &parts[1..];
            if tail.first() == Some(&"slot") {
                tail = &tail[1..]
            }
            let mut q = self.root.clone();
            for s in tail {
                q.push(s)
            }
            let q = q.canonicalize().ok()?;
            q.starts_with(&self.root).then_some(q)
        })();
        if self.path_cache.borrow().len() < 8192 {
            self.path_cache
                .borrow_mut()
                .insert(p.into(), result.clone());
        }
        result
    }

    fn exists(&self, p: &str) -> bool {
        !self.deleted.contains(p)
            && (self.files.contains_key(p) || self.dirs.contains(p) || self.asset(p).is_some())
    }

    fn is_dir(&self, p: &str) -> bool {
        !self.deleted.contains(p)
            && (self.dirs.contains(p) || self.asset(p).is_some_and(|v| v.is_dir()))
    }

    fn data(&self, p: &str) -> FsResult<Vec<u8>> {
        if self.deleted.contains(p) {
            return Err(2);
        }
        if let Some(v) = self.files.get(p) {
            return Ok(v.clone());
        }
        if let Some(v) = self.asset_cache.borrow().get(p) {
            return Ok(v.clone());
        }
        let q = self.asset(p).ok_or(2)?;
        if !q.is_file() {
            return Err(2);
        }
        if q.metadata().map_err(|_| 2)?.len() > MAX_FILE as u64 {
            return Err(27);
        }
        let b = std::fs::read(q).map_err(|_| 5)?;
        if self
            .asset_cache
            .borrow()
            .values()
            .map(Vec::len)
            .sum::<usize>()
            + b.len()
            <= 128 * 1024 * 1024
        {
            self.asset_cache.borrow_mut().insert(p.into(), b.clone());
        }
        Ok(b)
    }

    fn replace(&mut self, p: &str, b: Vec<u8>) -> FsResult<()> {
        if b.len() > MAX_FILE {
            return Err(27);
        }
        let total: usize = self
            .files
            .iter()
            .filter(|(k, _)| k.as_str() != p)
            .map(|(_, v)| v.len())
            .sum();
        if total + b.len() > MAX_OVERLAY {
            return Err(28);
        }
        self.files.insert(p.into(), b);
        self.deleted.remove(p);
        Ok(())
    }

    fn alloc(&mut self, c: Cpu, n: u32) -> FsResult<u32> {
        let a = self.next;
        let end = (a.checked_add(n).ok_or(12)? + 31) & !31;
        if end > 0x4020f000 {
            return Err(12);
        }
        c.write(a, &vec![0; n as usize]).map_err(|_| 14)?;
        self.next = end;
        Ok(a)
    }

    fn partition(&mut self, c: Cpu, p: &str, create: bool) -> FsResult<u32> {
        let n = if p == "/" {
            "/".into()
        } else {
            format!(
                "/{}",
                p.trim_start_matches('/')
                    .split('/')
                    .next()
                    .unwrap_or_default()
            )
        };
        if let Some(v) = self.devices.get(&n) {
            return Ok(*v);
        }
        if !create {
            return Ok(0);
        }
        let d = self.alloc(c, 0xe70)?;
        let a = self.alloc(c, n.len() as u32 + 1)?;
        c.write(a, n.as_bytes()).map_err(|_| 14)?;
        self.devices.insert(n, d);
        for (o, v) in [
            (0, a),
            (8, 2048),
            (12, 64),
            (0x14, 1),
            (0x18, 1024),
            (0x88, self.devices.len() as u32),
            (0x98, 2048),
        ] {
            c.put(d + o, v)
        }
        Ok(d)
    }

    fn stat(&self, c: Cpu, p: &str, a: u32) -> FsResult<u32> {
        if !self.exists(p) {
            return Err(2);
        }
        let dir = self.is_dir(p);
        let n = if dir { 0 } else { self.data(p)?.len() as u32 };
        let words = [
            0,
            1,
            if dir { 0o40755 } else { 0o100644 },
            1,
            0,
            0,
            0,
            n,
            2048,
            n.div_ceil(2048),
            0,
            0,
            0,
        ];
        let b: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
        c.write(a, &b).map_err(|_| 14)?;
        Ok(0)
    }

    pub fn operation(pc: u32) -> Option<&'static str> {
        Some(match pc {
            0x807f52fc => "add_partition",
            0x807f79d4 => "partition",
            0x807f7a1c | 0x807f7ae0 | 0x807f7b78 | 0x807f7ccc => "mount",
            0x807f7de0 | 0x807f66ec | 0x807f7864 => "sync",
            0x807f6238 | 0x807f66d4 => "open",
            0x807f67b0 => "close",
            0x807f6998 => "read",
            0x807f69b8 => "pread",
            0x807f6bac => "write",
            0x807f6bcc => "pwrite",
            0x807f6d30 => "seek",
            0x807f71a0 | 0x807f71b8 => "stat",
            0x807f71d0 => "fstat",
            0x807f773c => "access",
            0x807f78d8 => "mkdir",
            0x807f6ed8 => "unlink",
            0x807f6ec0 => "rmdir",
            0x807f6ef0 => "rename",
            0x807f8288 => "symlink",
            0x807f8338 => "readlink",
            0x807f809c => "opendir",
            0x807f8158 => "readdir",
            0x807f824c => "closedir",
            0x807f7fa0 => "rewinddir",
            0x807f77e8 => "chmod",
            _ => return None,
        })
    }

    pub fn dispatch(&mut self, c: Cpu, pc: u32) -> bool {
        let Some(op) = Self::operation(pc) else {
            return false;
        };
        self.calls += 1;
        let a = [c.reg(R0), c.reg(R0 + 1), c.reg(R0 + 2), c.reg(R0 + 3)];
        let v = match self.perform(c, op, a) {
            Ok(v) => v,
            Err(e) => {
                let thread = c.word(0x810da120);
                if (0x80000000..0x87ffff00).contains(&thread) {
                    c.put(thread + 0xc4, (-e) as u32)
                }
                if ["opendir", "readdir", "partition"].contains(&op) {
                    0
                } else {
                    u32::MAX
                }
            }
        };
        c.set(R0, v);
        c.set(PC, c.reg(LR));
        true
    }

    fn perform(&mut self, c: Cpu, op: &str, a: [u32; 4]) -> FsResult<u32> {
        if ["sync", "mount", "chmod"].contains(&op) {
            return Ok(0);
        }
        if op == "open" {
            let p = self.resolve(&Self::text(c, a[0])?)?;
            let present = self.exists(&p);
            if present && a[1] & 0xc0 == 0xc0 {
                return Err(17);
            }
            if !present {
                if a[1] & 0x40 == 0 || !self.is_dir(parent(&p)) {
                    return Err(2);
                }
                self.replace(&p, Vec::new())?
            }
            if self.is_dir(&p) {
                return Err(21);
            }
            if a[1] & 0x200 != 0 && a[1] & 3 != 0 {
                self.replace(&p, Vec::new())?
            }
            let fd = (0..100).find(|f| !self.handles.contains_key(f)).ok_or(24)?;
            let pos = if a[1] & 0x400 != 0 {
                self.data(&p)?.len()
            } else {
                0
            };
            self.handles.insert(
                fd,
                File {
                    path: p,
                    pos,
                    flags: a[1],
                },
            );
            return Ok(fd);
        }
        if ["read", "pread", "write", "pwrite", "seek", "close", "fstat"].contains(&op) {
            let mut h = self.handles.get(&a[0]).cloned().ok_or(9)?;
            if op == "close" {
                self.handles.remove(&a[0]);
                return Ok(0);
            }
            if op == "fstat" {
                return self.stat(c, &h.path, a[1]);
            }
            let mut b = self.data(&h.path)?;
            if op == "seek" {
                let base = match a[2] {
                    0 => 0,
                    1 => h.pos,
                    2 => b.len(),
                    _ => return Err(22),
                };
                let pos = base as i64 + a[1] as i32 as i64;
                if !(0..=MAX_FILE as i64).contains(&pos) {
                    return Err(22);
                }
                h.pos = pos as usize;
                self.handles.insert(a[0], h);
                return Ok(pos as u32);
            }
            let mut pos = if op == "pread" || op == "pwrite" {
                a[3] as usize
            } else {
                h.pos
            };
            let n = a[2] as usize;
            if n > MAX_FILE {
                return Err(27);
            }
            let count;
            if op == "read" || op == "pread" {
                if h.flags & 3 == 1 {
                    return Err(9);
                }
                let start = pos.min(b.len());
                let end = pos.saturating_add(n).min(b.len());
                count = end - start;
                if count > 0 {
                    c.write(a[1], &b[start..end]).map_err(|_| 14)?
                }
                self.reads += 1;
            } else {
                if h.flags & 3 == 0 {
                    return Err(9);
                }
                if h.flags & 0x400 != 0 && op == "write" {
                    pos = b.len()
                }
                let end = pos.checked_add(n).ok_or(27)?;
                if end > MAX_FILE {
                    return Err(27);
                }
                let bytes = c.read(a[1], n).map_err(|_| 14)?;
                if end > b.len() {
                    b.resize(end, 0)
                }
                b[pos..end].copy_from_slice(&bytes);
                self.replace(&h.path, b)?;
                count = n;
                self.writes += 1;
            }
            if op == "read" || op == "write" {
                h.pos = pos + count;
                self.handles.insert(a[0], h);
            }
            return Ok(count as u32);
        }
        if ["readdir", "closedir", "rewinddir"].contains(&op) {
            if !self.directories.contains_key(&a[0]) {
                return Err(9);
            }
            if op == "closedir" {
                self.directories.remove(&a[0]);
                self.free_dirs.push(a[0]);
                return Ok(0);
            }
            let d = self.directories.get_mut(&a[0]).unwrap();
            if op == "rewinddir" {
                d.pos = 0;
                return Ok(0);
            }
            let name = d.names.get(d.pos).cloned();
            d.pos += 1;
            let Some(name) = name else { return Ok(0) };
            c.write(a[0] + 4, &[0; 552]).map_err(|_| 14)?;
            c.write(a[0] + 15, &name.as_bytes()[..name.len().min(255)])
                .map_err(|_| 14)?;
            return Ok(a[0] + 4);
        }
        let raw = Self::text(c, a[0])?;
        let path = self.resolve(&raw)?;
        match op {
            "add_partition" => {
                self.partition(c, &path, true)?;
                self.dirs.insert(path);
                Ok(0)
            }
            "partition" => self.partition(c, &path, false),
            "stat" => self.stat(c, &path, a[1]),
            "access" => {
                if self.exists(&path) {
                    Ok(0)
                } else {
                    Err(2)
                }
            }
            "mkdir" => {
                if self.exists(&path) {
                    return Err(17);
                }
                if !self.is_dir(parent(&path)) {
                    return Err(2);
                }
                self.dirs.insert(path);
                Ok(0)
            }
            "symlink" => {
                self.links.insert(normalize(&Self::text(c, a[1])?)?, raw);
                Ok(0)
            }
            "readlink" => {
                let p = normalize(&raw)?;
                let t = self.links.get(&p).ok_or(22)?;
                let n = t.len().min(a[2] as usize);
                c.write(a[1], &t.as_bytes()[..n]).map_err(|_| 14)?;
                Ok(n as u32)
            }
            "unlink" | "rmdir" => {
                if !self.exists(&path) {
                    return Err(2);
                }
                self.deleted.insert(path.clone());
                self.files.remove(&path);
                self.dirs.remove(&path);
                Ok(0)
            }
            "rename" => {
                let dest = self.resolve(&Self::text(c, a[1])?)?;
                self.replace(&dest, self.data(&path)?)?;
                self.deleted.insert(path.clone());
                self.files.remove(&path);
                Ok(0)
            }
            "opendir" => {
                if !self.exists(&path) {
                    return Err(2);
                }
                if !self.is_dir(&path) {
                    return Err(20);
                }
                let mut names = BTreeSet::new();
                if let Some(asset) = self.asset(&path) {
                    if let Ok(entries) = std::fs::read_dir(asset) {
                        for q in entries.flatten() {
                            names.insert(q.file_name().to_string_lossy().into_owned());
                        }
                    }
                }
                for q in self
                    .files
                    .keys()
                    .chain(self.dirs.iter())
                    .chain(self.links.keys())
                {
                    if parent(q) == path && !self.deleted.contains(q) {
                        names.insert(q.rsplit('/').next().unwrap_or_default().to_owned());
                    }
                }
                let handle = if let Some(v) = self.free_dirs.pop() {
                    v
                } else {
                    self.alloc(c, 556)?
                };
                self.directories.insert(
                    handle,
                    Directory {
                        names: names.into_iter().collect(),
                        pos: 0,
                    },
                );
                Ok(handle)
            }
            _ => Err(38),
        }
    }
}

fn parent(s: &str) -> &str {
    s.rsplit_once('/')
        .map(|(p, _)| if p.is_empty() { "/" } else { p })
        .unwrap_or("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_cannot_escape_assets() {
        assert_eq!(normalize("slot/a").unwrap(), "/slot/a");
        assert_eq!(normalize("/slot/../secret"), Err(22));
        assert_eq!(normalize("C:\\Windows"), Err(22));
        assert_eq!(
            normalize("/slot//fonts/./font").unwrap(),
            "/slot/fonts/font"
        );
    }
}
