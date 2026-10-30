//! Whole owned-volume archives with Unix metadata and overlay deletion markers, streamed by the static node helper.

#[cfg(target_os = "linux")]
#[allow(
    unsafe_code,
    reason = "the trusted one-shot volume helper preserves Unix owners, device nodes, times and extended attributes with Linux syscalls"
)]
mod linux {
    use std::collections::BTreeMap;
    use std::ffi::{CString, OsString};
    use std::fs::{File, Metadata};
    use std::io::{Read, Write};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
    use std::path::{Component, Path, PathBuf};

    use anyhow::{Context, bail};
    use nix::libc;
    use serde::{Deserialize, Serialize};

    /// A versioned, length-delimited metadata stream; file bytes follow their header without buffering.
    const MAGIC: &[u8] = b"ORCH-VOLUME-1\n";
    /// Linux xattrs and paths are much smaller; this also bounds a corrupt archive's metadata allocation.
    const HEADER_MAX: usize = 4 * 1024 * 1024;

    /// The link accepts at most one update a second; counters remain independent of tree size.
    const REPORT_EVERY: std::time::Duration = std::time::Duration::from_secs(1);
    /// A fixed copy buffer keeps large files streaming while reporting within a file.
    const COPY_BUFFER_BYTES: usize = 64 * 1024;

    struct Counter {
        value: orch_driver_contract::volumes::VolumeProgress,
        last: std::time::Instant,
    }
    impl Counter {
        fn new(message: &str) -> Self {
            Self {
                value: orch_driver_contract::volumes::VolumeProgress {
                    directories_done: Some(0),
                    message: Some(message.into()),
                    ..Default::default()
                },
                last: std::time::Instant::now(),
            }
        }
        fn report(&mut self, force: bool) {
            if (force || self.last.elapsed() >= REPORT_EVERY)
                && let Ok(value) = serde_json::to_string(&self.value)
            {
                eprintln!("orch-progress {value}");
                self.last = std::time::Instant::now();
            }
        }
        fn directory(&mut self) {
            self.value.directories_done = self
                .value
                .directories_done
                .map(|value| value.saturating_add(1));
            self.report(false);
        }
        fn file(&mut self) {
            self.value.files_done = self.value.files_done.saturating_add(1);
            self.report(false);
        }
        fn bytes(&mut self, bytes: u64) {
            self.value.bytes_done = self
                .value
                .bytes_done
                .saturating_add(i64::try_from(bytes).unwrap_or(i64::MAX));
            self.report(false);
        }
    }
    fn copy_counted(
        input: &mut impl Read,
        output: &mut impl Write,
        count: &mut Counter,
    ) -> std::io::Result<u64> {
        let mut buffer = [0u8; COPY_BUFFER_BYTES];
        let mut total = 0u64;
        loop {
            let n = input.read(&mut buffer)?;
            if n == 0 {
                return Ok(total);
            }
            output.write_all(&buffer[..n])?;
            total = total.saturating_add(n as u64);
            count.bytes(n as u64);
        }
    }

    // A full home choice includes the persistent recording store, but a command credential
    // and an unfinished root-level download never acquire the lifetime of the carried home.
    fn transient_store_entry(root: &Path, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(root) else {
            return false;
        };
        let parts = relative
            .components()
            .map(|part| part.as_os_str())
            .collect::<Vec<_>>();
        parts.len() >= 3
            && parts[0].as_bytes() == b".orch"
            && parts[1].as_bytes() == b"file-states"
            && (parts[2].as_bytes().starts_with(b"source-credential-")
                || parts[2].as_bytes().starts_with(b".tmp"))
    }

    #[derive(Debug, Clone, Copy, Serialize, Deserialize)]
    #[serde(rename_all = "snake_case")]
    enum Kind {
        Directory,
        File,
        Link,
        HardLink,
        Character,
        Block,
        Fifo,
        Socket,
    }

    #[derive(Debug, Serialize, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Entry {
        path: Vec<u8>,
        kind: Kind,
        size: u64,
        uid: u32,
        gid: u32,
        mode: u32,
        atime: i64,
        atime_ns: i64,
        mtime: i64,
        mtime_ns: i64,
        link: Option<Vec<u8>>,
        major: u64,
        minor: u64,
        xattrs: Vec<(Vec<u8>, Vec<u8>)>,
    }

    fn name(path: &Path) -> anyhow::Result<CString> {
        Ok(CString::new(path.as_os_str().as_bytes())?)
    }

    fn checked(result: libc::c_int) -> std::io::Result<()> {
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn xattrs(path: &Path) -> anyhow::Result<Vec<(Vec<u8>, Vec<u8>)>> {
        let path = name(path)?;
        // SAFETY: the path is a live NUL-terminated string; a null buffer queries its length.
        let size = unsafe { libc::llistxattr(path.as_ptr(), std::ptr::null_mut(), 0) };
        if size < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EOPNOTSUPP) {
                return Ok(Vec::new());
            }
            return Err(error.into());
        }
        let mut names = vec![0u8; usize::try_from(size)?];
        // SAFETY: the allocated buffer has exactly the advertised capacity and remains live for the call.
        let count =
            unsafe { libc::llistxattr(path.as_ptr(), names.as_mut_ptr().cast(), names.len()) };
        if count < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        names.truncate(usize::try_from(count)?);
        let mut attributes = Vec::new();
        for key in names.split(|byte| *byte == 0).filter(|key| !key.is_empty()) {
            let key_c = CString::new(key)?;
            // SAFETY: both C strings are live, and a null value buffer queries the attribute's length.
            let size =
                unsafe { libc::lgetxattr(path.as_ptr(), key_c.as_ptr(), std::ptr::null_mut(), 0) };
            if size < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let mut value = vec![0u8; usize::try_from(size)?];
            // SAFETY: the value buffer has the queried capacity; it is not used beyond the returned length.
            let count = unsafe {
                libc::lgetxattr(
                    path.as_ptr(),
                    key_c.as_ptr(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                )
            };
            if count < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            value.truncate(usize::try_from(count)?);
            attributes.push((key.to_vec(), value));
        }
        Ok(attributes)
    }

    fn header(
        root: &Path,
        path: &Path,
        meta: &Metadata,
        hard_links: &mut BTreeMap<(u64, u64), Vec<u8>>,
    ) -> anyhow::Result<Entry> {
        let relative = path.strip_prefix(root)?.as_os_str().as_bytes().to_vec();
        let file_type = meta.file_type();
        let mut link = None;
        let kind = if file_type.is_dir() {
            Kind::Directory
        } else if file_type.is_symlink() {
            link = Some(std::fs::read_link(path)?.as_os_str().as_bytes().to_vec());
            Kind::Link
        } else if file_type.is_file() {
            if meta.nlink() > 1 {
                match hard_links.get(&(meta.dev(), meta.ino())) {
                    Some(first) => {
                        link = Some(first.clone());
                        Kind::HardLink
                    }
                    None => {
                        hard_links.insert((meta.dev(), meta.ino()), relative.clone());
                        Kind::File
                    }
                }
            } else {
                Kind::File
            }
        } else if file_type.is_char_device() {
            Kind::Character
        } else if file_type.is_block_device() {
            Kind::Block
        } else if file_type.is_fifo() {
            Kind::Fifo
        } else if file_type.is_socket() {
            Kind::Socket
        } else {
            bail!("the volume contains a file kind that cannot be preserved")
        };
        let size = if matches!(kind, Kind::File) {
            meta.len()
        } else {
            0
        };
        Ok(Entry {
            path: relative,
            kind,
            size,
            uid: meta.uid(),
            gid: meta.gid(),
            mode: meta.mode() & 0o7777,
            atime: meta.atime(),
            atime_ns: meta.atime_nsec(),
            mtime: meta.mtime(),
            mtime_ns: meta.mtime_nsec(),
            link,
            major: rustix::fs::major(meta.rdev()).into(),
            minor: rustix::fs::minor(meta.rdev()).into(),
            xattrs: xattrs(path)?,
        })
    }

    fn write_tree(
        root: &Path,
        path: &Path,
        out: &mut impl Write,
        hard_links: &mut BTreeMap<(u64, u64), Vec<u8>>,
        count: &mut Counter,
    ) -> anyhow::Result<()> {
        if transient_store_entry(root, path) {
            return Ok(());
        }
        let meta = std::fs::symlink_metadata(path)?;
        let entry = header(root, path, &meta, hard_links)?;
        let encoded = serde_json::to_vec(&entry)?;
        if encoded.len() > HEADER_MAX {
            bail!("the volume entry's metadata exceeds the archive bound")
        }
        out.write_all(&u32::try_from(encoded.len())?.to_le_bytes())?;
        out.write_all(&encoded)?;
        if matches!(entry.kind, Kind::File) {
            let mut file = File::open(path)?.take(entry.size);
            if copy_counted(&mut file, out, count)? != entry.size {
                bail!("a volume file changed while it was being carried")
            }
        }
        if meta.is_file() {
            count.file();
        }
        if meta.is_dir() {
            count.directory();
            for child in std::fs::read_dir(path)? {
                write_tree(root, &child?.path(), out, hard_links, count)?;
            }
        }
        Ok(())
    }

    pub(super) fn export(root: &Path, out: &mut impl Write) -> anyhow::Result<()> {
        out.write_all(MAGIC)?;
        let mut count = Counter::new("Archiving files");
        write_tree(root, root, out, &mut BTreeMap::new(), &mut count)?;
        count.report(true);
        out.write_all(&0u32.to_le_bytes())?;
        out.flush()?;
        Ok(())
    }

    pub(super) fn export_parts(
        root: &Path,
        parts: &[&str],
        out: &mut impl Write,
    ) -> anyhow::Result<()> {
        out.write_all(MAGIC)?;
        let mut links = BTreeMap::new();
        let mut count = Counter::new("Archiving recorded states");
        let meta = std::fs::symlink_metadata(root)?;
        let entry = header(root, root, &meta, &mut links)?;
        let encoded = serde_json::to_vec(&entry)?;
        if encoded.len() > HEADER_MAX {
            bail!("the store root metadata exceeds its archive bound")
        }
        out.write_all(&u32::try_from(encoded.len())?.to_le_bytes())?;
        out.write_all(&encoded)?;
        for part in parts {
            let path = root.join(part);
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => write_tree(root, &path, out, &mut links, &mut count)?,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => anyhow::bail!("a persistent store part is not an owned directory"),
            }
        }
        out.write_all(&0u32.to_le_bytes())?;
        out.flush()?;
        Ok(())
    }

    fn relative(bytes: &[u8]) -> anyhow::Result<PathBuf> {
        let path = PathBuf::from(OsString::from_vec(bytes.to_vec()));
        if bytes.contains(&0)
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            bail!("a carried volume path must stay below its owned root")
        }
        Ok(path)
    }

    fn inside(root: &Path, relative: &Path) -> anyhow::Result<PathBuf> {
        // Entries are restored parent first into an otherwise unused volume. No archive link is followed.
        let mut path = root.to_path_buf();
        let components: Vec<_> = relative.components().collect();
        for (index, component) in components.iter().enumerate() {
            path.push(component.as_os_str());
            if index + 1 < components.len() && !std::fs::symlink_metadata(&path)?.is_dir() {
                bail!("a carried path has a link or non-directory parent")
            }
        }
        Ok(path)
    }

    fn metadata(path: &Path, entry: &Entry) -> anyhow::Result<()> {
        let path_c = name(path)?;
        // SAFETY: only this unused owned volume is writable by the helper; lchown never follows a link.
        checked(unsafe { libc::lchown(path_c.as_ptr(), entry.uid, entry.gid) })?;
        if !matches!(entry.kind, Kind::Link) {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(entry.mode))?;
        }
        for (key, value) in &entry.xattrs {
            let key = CString::new(key.as_slice())?;
            // SAFETY: all buffers are live for the call; lsetxattr preserves a link's own attributes.
            checked(unsafe {
                libc::lsetxattr(
                    path_c.as_ptr(),
                    key.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                    0,
                )
            })?;
        }
        let times = [
            libc::timespec {
                tv_sec: entry.atime,
                tv_nsec: entry.atime_ns,
            },
            libc::timespec {
                tv_sec: entry.mtime,
                tv_nsec: entry.mtime_ns,
            },
        ];
        // SAFETY: a two-element timespec array and a valid path are passed, without following symlinks.
        checked(unsafe {
            libc::utimensat(
                libc::AT_FDCWD,
                path_c.as_ptr(),
                times.as_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        })?;
        Ok(())
    }

    pub(super) fn import(root: &Path, input: &mut impl Read) -> anyhow::Result<()> {
        let mut magic = vec![0u8; MAGIC.len()];
        input.read_exact(&mut magic)?;
        if magic != MAGIC {
            bail!("the volume archive version is not supported")
        }
        // Retrying an import replaces only its still-unused destination, never a source or live unit.
        for child in std::fs::read_dir(root)? {
            let child = child?;
            if child.file_type()?.is_dir() {
                std::fs::remove_dir_all(child.path())?;
            } else {
                std::fs::remove_file(child.path())?;
            }
        }
        let mut count = Counter::new("Unpacking files");
        let mut directories = Vec::new();
        let mut first = true;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let mut length = [0u8; 4];
            input.read_exact(&mut length)?;
            let length = usize::try_from(u32::from_le_bytes(length))?;
            if length == 0 {
                break;
            }
            if length > HEADER_MAX {
                bail!("the carried metadata exceeds the archive bound")
            }
            let mut encoded = vec![0u8; length];
            input.read_exact(&mut encoded)?;
            let entry: Entry = serde_json::from_slice(&encoded)?;
            let relative = relative(&entry.path)?;
            if first && (!relative.as_os_str().is_empty() || !matches!(entry.kind, Kind::Directory))
            {
                bail!("a volume archive begins with its root directory")
            }
            if !first && relative.as_os_str().is_empty() {
                bail!("a volume archive names its root once")
            }
            first = false;
            if !seen.insert(relative.clone()) {
                bail!("a volume archive repeats a path")
            }
            let path = inside(root, &relative)?;
            if !relative.as_os_str().is_empty() {
                match entry.kind {
                    Kind::Directory => std::fs::create_dir(&path)?,
                    Kind::File => {
                        let mut file = std::fs::OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&path)?;
                        if copy_counted(&mut input.take(entry.size), &mut file, &mut count)?
                            != entry.size
                        {
                            bail!("the volume archive ended inside a file")
                        }
                        file.sync_all()?;
                    }
                    Kind::Link => std::os::unix::fs::symlink(
                        OsString::from_vec(
                            entry.link.clone().context("a carried link has no target")?,
                        ),
                        &path,
                    )?,
                    Kind::HardLink => {
                        let target = relative_path(
                            entry
                                .link
                                .as_deref()
                                .context("a carried hard link has no target")?,
                        )?;
                        if !seen.contains(&target) || target == relative {
                            bail!("a carried hard link does not name an earlier file")
                        }
                        let target = inside(root, &target)?;
                        if !std::fs::symlink_metadata(&target)?.is_file() {
                            bail!("a carried hard link does not name a regular file")
                        }
                        std::fs::hard_link(target, &path)?;
                    }
                    Kind::Character | Kind::Block | Kind::Fifo => {
                        let kind = match entry.kind {
                            Kind::Character => libc::S_IFCHR,
                            Kind::Block => libc::S_IFBLK,
                            _ => libc::S_IFIFO,
                        };
                        let path_c = name(&path)?;
                        let device = rustix::fs::makedev(
                            u32::try_from(entry.major)?,
                            u32::try_from(entry.minor)?,
                        );
                        // SAFETY: creates only the checked unused-volume path with the archived device numbers.
                        checked(unsafe {
                            libc::mknod(path_c.as_ptr(), kind | entry.mode, device)
                        })?;
                    }
                    Kind::Socket => {
                        drop(std::os::unix::net::UnixListener::bind(&path)?);
                    }
                }
            }
            if matches!(entry.kind, Kind::File | Kind::HardLink) {
                count.file();
            }
            if matches!(entry.kind, Kind::Directory) {
                count.directory();
                directories.push((path, entry));
            } else {
                metadata(&path, &entry)?;
            }
        }
        if first {
            bail!("the volume archive has no root")
        }
        let mut extra = [0u8; 1];
        if input.read(&mut extra)? != 0 {
            bail!("the volume archive has trailing bytes")
        }
        for (path, entry) in directories.into_iter().rev() {
            metadata(&path, &entry)?;
        }
        count.report(true);
        Ok(())
    }

    pub(super) fn finish_store_parent(root: &Path) -> anyhow::Result<()> {
        let parent = root
            .parent()
            .ok_or_else(|| anyhow::anyhow!("the imported store has no parent"))?;
        let meta = std::fs::symlink_metadata(root)?;
        if !meta.is_dir() || !std::fs::symlink_metadata(parent)?.is_dir() {
            bail!("the store parent is not an owned directory")
        }
        let mut entry = header(root, root, &meta, &mut BTreeMap::new())?;
        // A newly made .orch parent must be writable by the owner restored on its private store.
        // Parent contents and extended attributes are not copied from this child directory.
        entry.xattrs.clear();
        metadata(parent, &entry)
    }

    fn relative_path(bytes: &[u8]) -> anyhow::Result<PathBuf> {
        relative(bytes)
    }

    pub(super) fn measure(
        root: &Path,
    ) -> anyhow::Result<orch_driver_contract::volumes::VolumeMeasure> {
        // Keep only one directory cursor per depth. A wide directory never becomes a Vec of names,
        // and an explicit stack avoids a call-stack limit on a deeply nested hierarchy.
        let mut count = Counter::new("Counting files");
        let mut stack: Vec<std::fs::ReadDir> = Vec::new();
        let mut next = Some(root.to_path_buf());
        loop {
            let path = match next.take() {
                Some(path) => path,
                None => {
                    let Some(directory) = stack.last_mut() else {
                        break;
                    };
                    match std::iter::Iterator::next(directory) {
                        Some(child) => child?.path(),
                        None => {
                            stack.pop();
                            continue;
                        }
                    }
                }
            };
            if transient_store_entry(root, &path) {
                continue;
            }
            let meta = std::fs::symlink_metadata(&path)?;
            if meta.is_file() {
                count.file();
                count.bytes(meta.len());
            } else if meta.is_dir() {
                count.directory();
                stack.push(std::fs::read_dir(&path)?);
            }
        }
        count.report(true);
        Ok(orch_driver_contract::volumes::VolumeMeasure {
            file_count: count.value.files_done,
            total_bytes: count.value.bytes_done,
            file_states: None,
        })
    }

    /// Fingerprints names, kinds, modes and bytes; timestamps and host-specific owners are not content.
    pub(super) fn fingerprint(root: &Path) -> anyhow::Result<orch_types::Hash> {
        use orch_types::Hash;
        use rustix::fs::{AtFlags, FileType, Mode, OFlags, open, openat, readlinkat, statat};
        use std::os::fd::AsRawFd;

        // The framing is versioned and length-delimited so distinct path/byte sequences cannot alias.
        const DOMAIN: &[u8] = b"ORCH-MOUNT-FINGERPRINT-1\0";
        // Content is streamed with a fixed scratch buffer, independent of any file's size.
        const READ_BYTES: usize = 64 * 1024;
        // Only the Unix permission and special mode bits belong to a content comparison.
        const MODE_BITS: u32 = 0o7777;

        fn unchanged(before: &Metadata, after: &Metadata) -> bool {
            before.dev() == after.dev()
                && before.ino() == after.ino()
                && before.mode() == after.mode()
                && before.len() == after.len()
                && before.mtime() == after.mtime()
                && before.mtime_nsec() == after.mtime_nsec()
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec()
        }

        fn scan(
            directory: &File,
            path: &Path,
            entries: &mut BTreeMap<Vec<u8>, Hash>,
            buffer: &mut [u8],
            count: &mut Counter,
        ) -> anyhow::Result<()> {
            let before = directory.metadata()?;
            count.directory();
            let mut identity = Hash::hasher();
            identity.update(b"directory\0");
            identity.update(&(before.mode() & MODE_BITS).to_le_bytes());
            entries.insert(
                path.as_os_str().as_bytes().to_vec(),
                Hash::finish(&identity),
            );
            // Readdir uses this held descriptor, never a mutable parent path or a directory symlink.
            for entry in std::fs::read_dir(format!("/proc/self/fd/{}", directory.as_raw_fd()))? {
                let name = entry?.file_name();
                let relative = path.join(&name);
                let meta = statat(directory, &name, AtFlags::SYMLINK_NOFOLLOW)?;
                let mut identity = Hash::hasher();
                match FileType::from_raw_mode(meta.st_mode) {
                    FileType::Directory => {
                        let child = File::from(openat(
                            directory,
                            &name,
                            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                            Mode::empty(),
                        )?);
                        let held = child.metadata()?;
                        if held.dev() != meta.st_dev || held.ino() != meta.st_ino {
                            bail!("a mount directory changed while it was being checked")
                        }
                        scan(&child, &relative, entries, buffer, count)?;
                        continue;
                    }
                    FileType::RegularFile => {
                        // Pin the inode without opening a device or pipe if the name was replaced after stat.
                        let held = File::from(openat(
                            directory,
                            &name,
                            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                            Mode::empty(),
                        )?);
                        let before = held.metadata()?;
                        if !before.is_file()
                            || before.dev() != meta.st_dev
                            || before.ino() != meta.st_ino
                        {
                            bail!("a mount file changed while it was being checked")
                        }
                        let mut file = File::open(format!("/proc/self/fd/{}", held.as_raw_fd()))?;
                        let mut content = Hash::hasher();
                        let mut size = 0u64;
                        loop {
                            let read = file.read(buffer)?;
                            if read == 0 {
                                break;
                            }
                            content.update(&buffer[..read]);
                            count.bytes(u64::try_from(read)?);
                            size = size
                                .checked_add(u64::try_from(read)?)
                                .context("a mount file is too large")?;
                        }
                        if size != before.len() || !unchanged(&before, &file.metadata()?) {
                            bail!("a mount file changed while it was being checked")
                        }
                        identity.update(b"file\0");
                        identity.update(&size.to_le_bytes());
                        identity.update(Hash::finish(&content).as_bytes());
                        count.file();
                    }
                    FileType::Symlink => {
                        identity.update(b"link\0");
                        let target = readlinkat(directory, &name, Vec::new())?;
                        identity.update(target.to_bytes());
                        count.file();
                    }
                    _ => bail!(
                        "this mount contains a special file that cannot be checked as content"
                    ),
                }
                identity.update(&(meta.st_mode & MODE_BITS).to_le_bytes());
                let after = statat(directory, &name, AtFlags::SYMLINK_NOFOLLOW)?;
                if meta.st_ino != after.st_ino
                    || meta.st_dev != after.st_dev
                    || meta.st_mode != after.st_mode
                    || meta.st_size != after.st_size
                    || meta.st_mtime != after.st_mtime
                    || meta.st_mtime_nsec != after.st_mtime_nsec
                    || meta.st_ctime != after.st_ctime
                    || meta.st_ctime_nsec != after.st_ctime_nsec
                {
                    bail!("a mount entry changed while it was being checked")
                }
                entries.insert(
                    relative.as_os_str().as_bytes().to_vec(),
                    Hash::finish(&identity),
                );
            }
            if !unchanged(&before, &directory.metadata()?) {
                bail!("a mount directory changed while it was being checked")
            }
            Ok(())
        }

        let directory = File::from(open(
            root,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        let mut entries = BTreeMap::new();
        let mut buffer = vec![0u8; READ_BYTES];
        let mut count = Counter::new("Checking content");
        count.report(true);
        scan(
            &directory,
            Path::new(""),
            &mut entries,
            &mut buffer,
            &mut count,
        )?;
        let mut result = Hash::hasher();
        result.update(DOMAIN);
        for (path, hash) in entries {
            result.update(&u64::try_from(path.len())?.to_le_bytes());
            result.update(&path);
            result.update(hash.as_bytes());
        }
        count.report(true);
        Ok(Hash::finish(&result))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn the_cluster_model_and_native_helper_read_each_others_existing_archive_format() {
            use orch_testkit::doubles::cluster::{ClusterArchiveCodec, ClusterFile, VolumeCodec};
            let target = tempfile::tempdir().unwrap();
            let owner = std::fs::metadata(target.path()).unwrap();
            let witness = ClusterFile {
                data: b"a cross-driver witness".to_vec(),
                uid: u64::from(owner.uid()),
                gid: u64::from(owner.gid()),
                mode: 0o640,
                atime: 1710000000,
                atime_ns: 23,
                mtime: 1720000000,
                mtime_ns: 47,
                xattrs: BTreeMap::from([("user.proof".into(), vec![0, 255, 42])]),
            };
            let files = BTreeMap::from([("roots/original.json".into(), witness)]);
            let encoded = VolumeCodec.encode(&files).unwrap();
            import(target.path(), &mut encoded.as_slice()).unwrap();
            let mut native = Vec::new();
            export(target.path(), &mut native).unwrap();
            assert_eq!(VolumeCodec.decode(&native).unwrap(), files);
            assert_eq!(
                std::fs::read(target.path().join("roots/original.json")).unwrap(),
                b"a cross-driver witness"
            );
        }

        #[test]
        fn fingerprints_ignore_creation_order_and_times_but_detect_bytes_names_and_modes() {
            let source = tempfile::tempdir().unwrap();
            let target = tempfile::tempdir().unwrap();
            for (root, order) in [(source.path(), ["a", "b"]), (target.path(), ["b", "a"])] {
                for name in order {
                    std::fs::write(root.join(name), b"same").unwrap();
                }
            }
            let original = fingerprint(source.path()).unwrap();
            assert_eq!(original, fingerprint(target.path()).unwrap());
            std::fs::write(target.path().join("a"), b"sAme").unwrap();
            assert_ne!(original, fingerprint(target.path()).unwrap());
            std::fs::write(target.path().join("a"), b"same").unwrap();
            assert_eq!(original, fingerprint(target.path()).unwrap());
            std::fs::rename(target.path().join("a"), target.path().join("renamed")).unwrap();
            assert_ne!(original, fingerprint(target.path()).unwrap());
            std::fs::rename(target.path().join("renamed"), target.path().join("a")).unwrap();
            std::fs::set_permissions(
                target.path().join("a"),
                std::fs::Permissions::from_mode(0o700),
            )
            .unwrap();
            assert_ne!(original, fingerprint(target.path()).unwrap());
        }

        #[test]
        fn fingerprints_include_empty_directories_and_links_without_reading_their_targets() {
            let root = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(outside.path().join("data"), b"outside").unwrap();
            std::os::unix::fs::symlink(outside.path(), root.path().join("link")).unwrap();
            let original = fingerprint(root.path()).unwrap();
            std::fs::write(outside.path().join("data"), b"changed outside").unwrap();
            assert_eq!(original, fingerprint(root.path()).unwrap());
            std::fs::create_dir(root.path().join("empty")).unwrap();
            assert_ne!(original, fingerprint(root.path()).unwrap());
            let linked_root = root.path().join("link");
            assert!(fingerprint(&linked_root).is_err());
        }

        #[test]
        fn carrying_a_whole_home_keeps_recorded_data_without_a_commands_temporary_credential() {
            let source = tempfile::tempdir().unwrap();
            let target = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(
                source
                    .path()
                    .join(".orch/file-states/source-credential-interrupted"),
            )
            .unwrap();
            std::fs::create_dir_all(source.path().join(".orch/file-states/roots")).unwrap();
            std::fs::write(
                source
                    .path()
                    .join(".orch/file-states/source-credential-interrupted/password"),
                "COMMAND_SCOPED_TOKEN",
            )
            .unwrap();
            std::fs::write(
                source.path().join(".orch/file-states/.tmp-download"),
                "unfinished bytes",
            )
            .unwrap();
            std::fs::write(
                source.path().join(".orch/file-states/roots/kept"),
                "kept state metadata",
            )
            .unwrap();
            std::fs::write(source.path().join("home-file"), "kept home bytes").unwrap();
            let measured = measure(source.path()).unwrap();
            assert_eq!(measured.file_count, 2);
            let mut bytes = Vec::new();
            export(source.path(), &mut bytes).unwrap();
            assert!(
                !bytes
                    .windows(b"COMMAND_SCOPED_TOKEN".len())
                    .any(|part| part == b"COMMAND_SCOPED_TOKEN")
            );
            import(target.path(), &mut bytes.as_slice()).unwrap();
            assert_eq!(
                std::fs::read(target.path().join(".orch/file-states/roots/kept")).unwrap(),
                b"kept state metadata"
            );
            assert_eq!(
                std::fs::read(target.path().join("home-file")).unwrap(),
                b"kept home bytes"
            );
            assert!(
                !target
                    .path()
                    .join(".orch/file-states/source-credential-interrupted")
                    .exists()
            );
            assert!(
                !target
                    .path()
                    .join(".orch/file-states/.tmp-download")
                    .exists()
            );
            assert_eq!(
                measure(target.path()).unwrap().total_bytes,
                measured.total_bytes
            );
        }
        #[test]
        fn whole_home_stream_preserves_non_text_names_links_permissions_and_binary_extended_attributes()
         {
            let source = tempfile::tempdir().unwrap();
            let target = tempfile::tempdir().unwrap();
            let path = source
                .path()
                .join(OsString::from_vec(b"name-\xff".to_vec()));
            std::fs::write(&path, b"kept").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
            std::fs::hard_link(&path, source.path().join("same-file")).unwrap();
            std::os::unix::fs::symlink("/outside/is/not/read", source.path().join("link")).unwrap();
            let path_c = name(&path).unwrap();
            let key = CString::new("user.orch-test").unwrap();
            let value = b"binary\0value\xff";
            // SAFETY: this test owns the path and the attribute/value buffers.
            checked(unsafe {
                libc::lsetxattr(
                    path_c.as_ptr(),
                    key.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                    0,
                )
            })
            .unwrap();
            let mut bytes = Vec::new();
            export(source.path(), &mut bytes).unwrap();
            import(target.path(), &mut bytes.as_slice()).unwrap();
            let copied = target.path().join(path.file_name().unwrap());
            let old = std::fs::symlink_metadata(&path).unwrap();
            let new = std::fs::symlink_metadata(&copied).unwrap();
            assert_eq!(std::fs::read(&copied).unwrap(), b"kept");
            assert_eq!(
                (new.uid(), new.gid(), new.mode() & 0o7777),
                (old.uid(), old.gid(), 0o640)
            );
            assert_eq!(
                new.ino(),
                std::fs::metadata(target.path().join("same-file"))
                    .unwrap()
                    .ino()
            );
            assert_eq!(
                xattrs(&copied).unwrap(),
                vec![(b"user.orch-test".to_vec(), value.to_vec())]
            );
            assert_eq!(
                std::fs::read_link(target.path().join("link")).unwrap(),
                Path::new("/outside/is/not/read")
            );
            assert_eq!(measure(target.path()).unwrap().total_bytes, 8);
        }
    }
}

/// Counts a tree without following links or holding its directory listings in memory.
pub fn measure(
    root: &std::path::Path,
) -> anyhow::Result<orch_driver_contract::volumes::VolumeMeasure> {
    #[cfg(target_os = "linux")]
    {
        linux::measure(root)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = root;
        anyhow::bail!("whole volume measurements require Unix metadata support")
    }
}

/// The same trusted static helper entry point invokes these three volume operations.
pub fn run(operation: crate::runtime_helper::Operation) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        let root = std::path::Path::new("/volume");
        match operation {
            crate::runtime_helper::Operation::VolumeFingerprint => {
                println!("{}", serde_json::to_string(&linux::fingerprint(root)?)?);
                Ok(())
            }
            crate::runtime_helper::Operation::VolumeMeasure => {
                println!("{}", serde_json::to_string(&linux::measure(root)?)?);
                Ok(())
            }
            crate::runtime_helper::Operation::VolumeExport => {
                linux::export(root, &mut std::io::stdout().lock())
            }
            crate::runtime_helper::Operation::VolumeImport => {
                linux::import(root, &mut std::io::stdin().lock())
            }
            _ => anyhow::bail!("this is not a volume operation"),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = operation;
        anyhow::bail!("whole volume archives require Linux Unix metadata support")
    }
}

/// Measures an owned fixed subtree with the same no-link-following archive rules.
pub(crate) fn measure_parts(
    root: &std::path::Path,
    parts: &[&str],
) -> anyhow::Result<orch_driver_contract::volumes::VolumeMeasure> {
    #[cfg(target_os = "linux")]
    {
        let mut result = orch_driver_contract::volumes::VolumeMeasure {
            file_count: 0,
            total_bytes: 0,
            file_states: None,
        };
        for part in parts {
            let path = root.join(part);
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => {
                    let measured = linux::measure(&path)?;
                    result.file_count = result.file_count.saturating_add(measured.file_count);
                    result.total_bytes = result.total_bytes.saturating_add(measured.total_bytes);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => anyhow::bail!("a persistent store part is not an owned directory"),
            }
        }
        Ok(result)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, parts);
        anyhow::bail!("owned store carry requires Unix metadata support")
    }
}

/// Writes only known persistent format parts; temporary secrets never enter a state-store archive.
pub(crate) fn export_parts(
    root: &std::path::Path,
    parts: &[&str],
    output: &mut impl std::io::Write,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::export_parts(root, parts, output)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, parts, output);
        anyhow::bail!("owned store carry requires Unix metadata support")
    }
}

/// Imports a fixed owned subtree before its successor is started.
pub(crate) fn import_subtree(
    root: &std::path::Path,
    input: &mut impl std::io::Read,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        linux::import(root, input)?;
        linux::finish_store_parent(root)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (root, input);
        anyhow::bail!("owned store carry requires Unix metadata support")
    }
}
