use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::Path;
use std::time::{Duration, SystemTime};

use fuser::{
    FileAttr, FileType, Filesystem, KernelConfig, ReplyAttr, ReplyCreate, ReplyData,
    ReplyDirectory, ReplyEmpty, ReplyEntry, ReplyOpen, ReplyWrite, Request, TimeOrNow,
};
use libc::{c_int, EROFS};

use crate::error::TaprootError;
use crate::state::SignedState;

// ---------------------------------------------------------------------------
// Inode table
// ---------------------------------------------------------------------------

const TTL: Duration = Duration::from_secs(1);
const ROOT_INO: u64 = 1;

#[derive(Debug, Clone)]
struct Inode {
    ino: u64,
    parent: u64,
    name: String,
    kind: FileType,
    data: Vec<u8>,
    children: Vec<u64>,
}

fn now() -> SystemTime {
    SystemTime::now()
}

fn file_attr(ino: u64, size: u64, kind: FileType) -> FileAttr {
    let t = now();
    FileAttr {
        ino,
        size,
        blocks: size
            .div_ceil(512)
            .max(if kind == FileType::Directory { 1 } else { 0 }),
        atime: t,
        mtime: t,
        ctime: t,
        crtime: t,
        kind,
        perm: if kind == FileType::Directory {
            0o555
        } else {
            0o444
        },
        nlink: if kind == FileType::Directory { 2 } else { 1 },
        uid: unsafe { libc::getuid() } as u32,
        gid: unsafe { libc::getgid() } as u32,
        rdev: 0,
        blksize: 4096,
        flags: 0,
    }
}

fn is_safe_filename(name: &str) -> bool {
    if name.is_empty() || name.len() > 64 {
        return false;
    }
    if name == "." || name == ".." {
        return false;
    }
    if name.contains('/') || name.contains('\0') || name.contains('\\') {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

// ---------------------------------------------------------------------------
// TaprootFS
// ---------------------------------------------------------------------------

pub struct TaprootFS {
    inodes: HashMap<u64, Inode>,
    /// (parent_ino, name) -> ino
    lookup: HashMap<(u64, String), u64>,
    next_ino: u64,
}

impl TaprootFS {
    pub fn new(signed: &SignedState) -> Self {
        let mut fs = Self {
            inodes: HashMap::new(),
            lookup: HashMap::new(),
            next_ino: ROOT_INO + 1,
        };

        fs.inodes.insert(
            ROOT_INO,
            Inode {
                ino: ROOT_INO,
                parent: ROOT_INO,
                name: String::new(),
                kind: FileType::Directory,
                data: Vec::new(),
                children: Vec::new(),
            },
        );

        let readme = Self::readme_content(signed);
        let state_json = serde_json::to_string_pretty(signed).unwrap_or_else(|_| "{}".into());
        let env_content = Self::env_content(signed);

        fs.add_file(ROOT_INO, "README.taproot", readme.into_bytes());
        fs.add_file(ROOT_INO, "state.json", state_json.into_bytes());
        fs.add_file(ROOT_INO, "env", env_content.into_bytes());
        fs.add_file(ROOT_INO, "hash", signed.hash.clone().into_bytes());
        fs.add_file(
            ROOT_INO,
            "version",
            signed.state.version.clone().into_bytes(),
        );

        let runtimes_ino = fs.add_dir(ROOT_INO, "runtimes");
        for r in &signed.state.runtimes {
            if !is_safe_filename(&r.name) {
                continue;
            }
            let content = format!(
                "name: {}\nversion: {}\npinned: {}\n",
                r.name, r.version, r.pinned
            );
            let fname = format!("{}.txt", r.name);
            if fs.lookup.contains_key(&(runtimes_ino, fname.clone())) {
                continue;
            }
            fs.add_file(runtimes_ino, &fname, content.into_bytes());
        }

        let containers_ino = fs.add_dir(ROOT_INO, "containers");
        for c in &signed.state.containers {
            if !is_safe_filename(&c.name) {
                continue;
            }
            let content = format!(
                "name: {}\nversion: {}\nimage: {}\nsigned: {}\n",
                c.name, c.version, c.image, c.signed
            );
            let fname = format!("{}.txt", c.name);
            if fs.lookup.contains_key(&(containers_ino, fname.clone())) {
                continue;
            }
            fs.add_file(containers_ino, &fname, content.into_bytes());
        }

        fs
    }

    fn readme_content(signed: &SignedState) -> String {
        let s = &signed.state;
        let sig = if signed.signature.is_some() {
            "signed"
        } else {
            "unsigned"
        };
        format!(
            "taproot read-only mount\n\
             =======================\n\
             repo: {}  branch: {}  commit: {}\n\
             state: {sig}  sha256:{}\n\
             runtimes: {}  containers: {}  env-vars: {}\n\
             \n\
             This filesystem is read-only. All writes return EROFS.\n\
             Files:\n\
               state.json  — pretty-printed SignedState\n\
               env         — key=value list\n\
               hash        — sha256 hex\n\
               version     — schema version\n\
               runtimes/   — per-runtime virtual files\n\
               containers/ — per-container virtual files\n",
            s.base.repo,
            s.base.branch,
            s.base.commit,
            signed.hash,
            s.runtimes.len(),
            s.containers.len(),
            s.env_vars.len(),
        )
    }

    fn env_content(signed: &SignedState) -> String {
        if signed.state.env_vars.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        for (k, v) in &signed.state.env_vars {
            out.push_str(k);
            out.push('=');
            out.push_str(v);
            out.push('\n');
        }
        out
    }

    fn add_file(&mut self, parent: u64, name: &str, data: Vec<u8>) -> u64 {
        let ino = self.next_ino;
        self.next_ino += 1;
        let inode = Inode {
            ino,
            parent,
            name: name.to_string(),
            kind: FileType::RegularFile,
            data,
            children: Vec::new(),
        };
        self.inodes.insert(ino, inode);
        self.lookup.insert((parent, name.to_string()), ino);
        if let Some(p) = self.inodes.get_mut(&parent) {
            p.children.push(ino);
        }
        ino
    }

    fn add_dir(&mut self, parent: u64, name: &str) -> u64 {
        let ino = self.next_ino;
        self.next_ino += 1;
        let inode = Inode {
            ino,
            parent,
            name: name.to_string(),
            kind: FileType::Directory,
            data: Vec::new(),
            children: Vec::new(),
        };
        self.inodes.insert(ino, inode);
        self.lookup.insert((parent, name.to_string()), ino);
        if let Some(p) = self.inodes.get_mut(&parent) {
            p.children.push(ino);
        }
        ino
    }

    #[cfg(test)]
    pub fn inode_count(&self) -> usize {
        self.inodes.len()
    }

    #[cfg(test)]
    #[allow(private_interfaces)]
    pub fn get_inode(&self, ino: u64) -> Option<&Inode> {
        self.inodes.get(&ino)
    }

    #[cfg(test)]
    pub fn lookup_ino(&self, parent: u64, name: &str) -> Option<u64> {
        self.lookup.get(&(parent, name.to_string())).copied()
    }

    fn getattr_for(&self, ino: u64) -> Option<FileAttr> {
        let inode = self.inodes.get(&ino)?;
        let size = if inode.kind == FileType::Directory {
            0
        } else {
            inode.data.len() as u64
        };
        Some(file_attr(ino, size, inode.kind))
    }
}

// ---------------------------------------------------------------------------
// Filesystem impl — read-only
// ---------------------------------------------------------------------------

impl Filesystem for TaprootFS {
    fn init(&mut self, _req: &Request<'_>, _config: &mut KernelConfig) -> Result<(), c_int> {
        Ok(())
    }

    fn lookup(&mut self, _req: &Request<'_>, parent: u64, name: &OsStr, reply: ReplyEntry) {
        let name_str = name.to_string_lossy().to_string();
        if let Some(&ino) = self.lookup.get(&(parent, name_str)) {
            if let Some(attr) = self.getattr_for(ino) {
                reply.entry(&TTL, &attr, 0);
                return;
            }
        }
        reply.error(libc::ENOENT);
    }

    fn getattr(&mut self, _req: &Request<'_>, ino: u64, reply: ReplyAttr) {
        if let Some(attr) = self.getattr_for(ino) {
            reply.attr(&TTL, &attr);
        } else {
            reply.error(libc::ENOENT);
        }
    }

    fn open(&mut self, _req: &Request<'_>, ino: u64, flags: i32, reply: ReplyOpen) {
        if (flags & libc::O_TRUNC) != 0 || (flags & libc::O_CREAT) != 0 {
            reply.error(EROFS);
            return;
        }
        let accmode = flags & libc::O_ACCMODE;
        if accmode == libc::O_WRONLY || accmode == libc::O_RDWR {
            reply.error(EROFS);
            return;
        }
        if self.inodes.contains_key(&ino) {
            reply.opened(0, 0);
        } else {
            reply.error(libc::ENOENT);
        }
    }

    fn read(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        size: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyData,
    ) {
        let Some(inode) = self.inodes.get(&ino) else {
            reply.error(libc::ENOENT);
            return;
        };
        if inode.kind == FileType::Directory {
            reply.error(libc::EISDIR);
            return;
        }
        if offset < 0 {
            reply.error(libc::EINVAL);
            return;
        }
        let data = &inode.data;
        let off = offset as usize;
        if off >= data.len() {
            reply.data(&[]);
            return;
        }
        let end = (off + size as usize).min(data.len());
        reply.data(&data[off..end]);
    }

    fn readdir(
        &mut self,
        _req: &Request<'_>,
        ino: u64,
        _fh: u64,
        offset: i64,
        mut reply: ReplyDirectory,
    ) {
        if offset < 0 {
            reply.error(libc::EINVAL);
            return;
        }
        let Some(inode) = self.inodes.get(&ino).cloned() else {
            reply.error(libc::ENOENT);
            return;
        };
        if inode.kind != FileType::Directory {
            reply.error(libc::ENOTDIR);
            return;
        }

        let mut entries: Vec<(u64, FileType, String)> = Vec::new();
        entries.push((ino, FileType::Directory, ".".to_string()));
        entries.push((
            if ino == ROOT_INO {
                ROOT_INO
            } else {
                inode.parent
            },
            FileType::Directory,
            "..".to_string(),
        ));
        for &child_ino in &inode.children {
            if let Some(child) = self.inodes.get(&child_ino) {
                entries.push((child.ino, child.kind, child.name.clone()));
            }
        }

        for (i, (child_ino, kind, name)) in entries.into_iter().enumerate() {
            let idx = (i + 1) as i64;
            if idx <= offset {
                continue;
            }
            if reply.add(child_ino, idx, kind, name) {
                break;
            }
        }
        reply.ok();
    }

    fn release(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _flags: i32,
        _lock_owner: Option<u64>,
        _flush: bool,
        reply: ReplyEmpty,
    ) {
        reply.ok();
    }

    // --- read-only denials ---
    fn write(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _fh: u64,
        _offset: i64,
        _data: &[u8],
        _write_flags: u32,
        _flags: i32,
        _lock_owner: Option<u64>,
        reply: ReplyWrite,
    ) {
        reply.error(EROFS);
    }

    fn create(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        _flags: i32,
        reply: ReplyCreate,
    ) {
        reply.error(EROFS);
    }

    fn mkdir(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        reply: ReplyEntry,
    ) {
        reply.error(EROFS);
    }

    fn mknod(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        _name: &OsStr,
        _mode: u32,
        _umask: u32,
        _rdev: u32,
        reply: ReplyEntry,
    ) {
        reply.error(EROFS);
    }

    fn unlink(&mut self, _req: &Request<'_>, _parent: u64, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(EROFS);
    }

    fn rmdir(&mut self, _req: &Request<'_>, _parent: u64, _name: &OsStr, reply: ReplyEmpty) {
        reply.error(EROFS);
    }

    fn rename(
        &mut self,
        _req: &Request<'_>,
        _parent: u64,
        _name: &OsStr,
        _newparent: u64,
        _newname: &OsStr,
        _flags: u32,
        reply: ReplyEmpty,
    ) {
        reply.error(EROFS);
    }

    fn setattr(
        &mut self,
        _req: &Request<'_>,
        _ino: u64,
        _mode: Option<u32>,
        _uid: Option<u32>,
        _gid: Option<u32>,
        _size: Option<u64>,
        _atime: Option<TimeOrNow>,
        _mtime: Option<TimeOrNow>,
        _ctime: Option<SystemTime>,
        _fh: Option<u64>,
        _crtime: Option<SystemTime>,
        _chgtime: Option<SystemTime>,
        _bkuptime: Option<SystemTime>,
        _flags: Option<u32>,
        reply: ReplyAttr,
    ) {
        reply.error(EROFS);
    }
}

// ---------------------------------------------------------------------------
// Public mount helper
// ---------------------------------------------------------------------------

/// Mount a read-only FUSE filesystem at `mountpoint` reflecting `signed`.
///
/// Blocks until unmounted. Mount options: RO, FSName("taproot").
pub fn mount_readonly(mountpoint: &Path, signed: &SignedState) -> Result<(), TaprootError> {
    let meta = std::fs::symlink_metadata(mountpoint).map_err(|e| {
        TaprootError::Mount(format!(
            "mountpoint does not exist: {}: {e}",
            mountpoint.display()
        ))
    })?;
    if meta.file_type().is_symlink() {
        return Err(TaprootError::Mount(format!(
            "mountpoint is a symlink (refusing): {}",
            mountpoint.display()
        )));
    }
    if !meta.is_dir() {
        return Err(TaprootError::Mount(format!(
            "mountpoint is not a directory: {}",
            mountpoint.display()
        )));
    }
    let fs = TaprootFS::new(signed);
    let options = [
        fuser::MountOption::RO,
        fuser::MountOption::FSName("taproot".to_string()),
        fuser::MountOption::Subtype("taproot".to_string()),
    ];
    fuser::mount2(fs, mountpoint, &options).map_err(|e| TaprootError::Mount(e.to_string()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::TaprootState;

    fn sample_signed() -> SignedState {
        let state = TaprootState::new("myapp", "main", "abc123")
            .with_runtime("python", "3.11.4")
            .with_env("FOO", "bar")
            .with_container("postgres", "15.3", "postgres:15.3");
        let hash = crate::engine::StateEngine::hash(&state).unwrap();
        SignedState {
            state,
            hash,
            signature: None,
            public_key: None,
        }
    }

    #[test]
    fn inode_table_has_expected_files() {
        let signed = sample_signed();
        let fs = TaprootFS::new(&signed);
        assert!(fs.inode_count() >= 8);
        assert!(fs.lookup_ino(ROOT_INO, "state.json").is_some());
        assert!(fs.lookup_ino(ROOT_INO, "README.taproot").is_some());
        assert!(fs.lookup_ino(ROOT_INO, "env").is_some());
        assert!(fs.lookup_ino(ROOT_INO, "hash").is_some());
        assert!(fs.lookup_ino(ROOT_INO, "runtimes").is_some());
        assert!(fs.lookup_ino(ROOT_INO, "containers").is_some());
    }

    #[test]
    fn file_content_correct() {
        let signed = sample_signed();
        let fs = TaprootFS::new(&signed);
        let ino = fs.lookup_ino(ROOT_INO, "env").unwrap();
        let inode = fs.get_inode(ino).unwrap();
        let text = String::from_utf8_lossy(&inode.data);
        assert!(text.contains("FOO=bar"));

        let ino2 = fs.lookup_ino(ROOT_INO, "hash").unwrap();
        let inode2 = fs.get_inode(ino2).unwrap();
        assert_eq!(String::from_utf8_lossy(&inode2.data), signed.hash);
    }

    #[test]
    fn per_runtime_container_files() {
        let signed = sample_signed();
        let fs = TaprootFS::new(&signed);
        let runtimes = fs.lookup_ino(ROOT_INO, "runtimes").unwrap();
        assert!(fs.lookup_ino(runtimes, "python.txt").is_some());
        let containers = fs.lookup_ino(ROOT_INO, "containers").unwrap();
        assert!(fs.lookup_ino(containers, "postgres.txt").is_some());
    }

    #[test]
    fn getattr_root_is_dir() {
        let signed = sample_signed();
        let fs = TaprootFS::new(&signed);
        let attr = fs.getattr_for(ROOT_INO).unwrap();
        assert_eq!(attr.kind, FileType::Directory);
        assert_eq!(attr.perm, 0o555);
    }

    #[test]
    fn mount_readonly_errors_on_missing_path() {
        let signed = sample_signed();
        let res = mount_readonly(Path::new("/tmp/does-not-exist-taproot-test-xyz"), &signed);
        assert!(res.is_err());
        assert!(matches!(res.unwrap_err(), TaprootError::Mount(_)));
    }
}
