//! One-shot mount namespace operations for an owned Docker unit, using the node's static binary.

use clap::{Args as ClapArgs, ValueEnum};

/// The three operations performed by the same short-lived helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Operation {
    /// Clone the Docker-bound source and attach it at the target.
    Attach,
    /// Remove the mount from the target namespace.
    Detach,
    /// Give or remove a character device node.
    Device,
    /// Read only the kind, device numbers and permissions of Docker's selected source.
    Source,
    /// Read the owned unit's current PID namespace and first-process start identity.
    Identity,
    /// Query the stable KVM API version of the caller's own /dev/kvm; no virtual machine is created.
    KvmVersion,
    /// Wait for the node's read-only readiness file, then replace this process with the managed connector.
    Launch,
    /// Verify a pinned existing directory as the unit's user, without writing or changing metadata.
    DirectoryCheck,
    /// Measure one Docker-bound owned volume without reading file contents.
    VolumeMeasure,
    /// Fingerprint a read-only volume or pinned directory without following links.
    VolumeFingerprint,
    /// Stream the whole owned volume and its metadata to stdout.
    VolumeExport,
    /// Restore a whole archive from stdin into an unused owned volume.
    VolumeImport,
    /// Measure persistent recorded states independently of the rest of the home volume.
    FileStatesMeasure,
    /// Export the fixed file-state format without transient command credentials.
    FileStatesExport,
    /// Import persistent states into the unused successor's home before first start.
    FileStatesImport,
}

/// Inputs from the trusted node's Docker driver; the source is always Docker's bind at /src.
#[derive(Debug, Clone, ClapArgs)]
pub struct Args {
    /// Which operation to perform.
    #[arg(value_enum)]
    pub operation: Operation,
    /// The absolute target inside the unit.
    #[arg(long)]
    pub target: String,
    /// Recursively read-only for an attached source.
    #[arg(long)]
    pub read_only: bool,
    /// A file or socket target rather than a directory.
    #[arg(long)]
    pub file: bool,
    /// Remove the device instead of giving it.
    #[arg(long)]
    pub remove: bool,
    /// The device major allowed by this helper's device-cgroup rule.
    #[arg(long)]
    pub major: Option<u64>,
    /// The device minor allowed by this helper's device-cgroup rule.
    #[arg(long)]
    pub minor: Option<u64>,
    /// Actual workspace/home roots supplied from the owned container's immutable launch configuration.
    #[arg(long)]
    pub reserved: Vec<String>,
}

/// Executes once and returns. It is invoked before a Tokio runtime or any other thread exists.
pub fn run(args: &Args) -> anyhow::Result<()> {
    if matches!(
        args.operation,
        Operation::VolumeMeasure
            | Operation::VolumeFingerprint
            | Operation::VolumeExport
            | Operation::VolumeImport
    ) {
        check_directory_identities(&args.reserved)?;
        return crate::volume_archive::run(args.operation);
    }
    if matches!(
        args.operation,
        Operation::FileStatesMeasure | Operation::FileStatesExport | Operation::FileStatesImport
    ) {
        check_directory_identities(&args.reserved)?;
        return crate::file_state_volume::run(args.operation);
    }
    if args.operation == Operation::Launch {
        return launch(args);
    }
    if args.operation == Operation::DirectoryCheck {
        check_directory_bindings(&args.reserved)?;
        println!("{{}}");
        return Ok(());
    }
    if args.operation == Operation::Identity {
        println!("{}", serde_json::json!({"started":start_identity()?}));
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    if args.operation == Operation::Source {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        let metadata = std::fs::symlink_metadata("/src")?;
        let kind = if metadata.file_type().is_char_device() {
            "device"
        } else if metadata.file_type().is_socket() {
            "socket"
        } else {
            "other"
        };
        println!(
            "{}",
            serde_json::json!({"kind":kind,"major":rustix::fs::major(metadata.rdev()),"minor":rustix::fs::minor(metadata.rdev()),"gid":metadata.gid(),"mode":metadata.mode() & 0o777})
        );
        return Ok(());
    }
    orch_driver_contract::mounts::check_target(&args.target)?;
    if args.operation == Operation::Device {
        let name = args
            .target
            .strip_prefix("/dev/")
            .ok_or_else(|| anyhow::anyhow!("a device target is under /dev"))?;
        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            anyhow::bail!("a device target names one device");
        }
    }
    #[cfg(target_os = "linux")]
    {
        linux::run(args)
    }
    #[cfg(not(target_os = "linux"))]
    {
        anyhow::bail!("live namespace operations require Linux 5.12 or later")
    }
}

fn launch(args: &Args) -> anyhow::Result<()> {
    // Two thirty-second device/mount helpers and the start acknowledgment fit inside this
    // two-minute gate. It never enrolls with a partially restored grant or waits forever.
    let until = std::time::Instant::now() + std::time::Duration::from_secs(120);
    let identity = start_identity()?;
    while !std::fs::read_to_string("/run/orch-capability-gate/ready")
        .is_ok_and(|value| value == identity)
    {
        if std::time::Instant::now() >= until {
            anyhow::bail!("the node did not finish restoring this unit's capabilities");
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    check_directory_bindings(&args.reserved)?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(std::process::Command::new("/opt/orch/bin/orch-connector")
            .args(["run", "--managed"])
            .exec()
            .into())
    }
    #[cfg(not(unix))]
    {
        anyhow::bail!("managed capability launch requires Unix")
    }
}

fn check_directory_bindings(bindings: &[String]) -> anyhow::Result<()> {
    check_directory_identities(bindings)?;
    // The repeated arguments here are protected by Docker's immutable launch configuration.
    // They contain path/dev/inode objects, never strings supplied through the unit's environment.
    for expected in bindings {
        let expected: orch_driver_contract::runtime::DirectoryBinding =
            serde_json::from_str(expected)?;
        #[cfg(unix)]
        {
            orch_connector_core::policy::writable_environment_directory(std::path::Path::new(
                &expected.path,
            ))?;
        }
    }
    Ok(())
}

fn check_directory_identities(bindings: &[String]) -> anyhow::Result<()> {
    for expected in bindings {
        let expected: orch_driver_contract::runtime::DirectoryBinding =
            serde_json::from_str(expected)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(&expected.path)?;
            if !metadata.is_dir()
                || expected.device != Some(metadata.dev())
                || expected.inode != Some(metadata.ino())
            {
                anyhow::bail!(
                    "the existing directory at {} changed since environment.roots was checked; its owners must restore it or recreate with an allowed directory",
                    expected.path
                );
            }
        }
    }
    Ok(())
}

fn start_identity() -> anyhow::Result<String> {
    let stat = std::fs::read_to_string("/proc/1/stat")?;
    let fields = stat
        .rsplit_once(')')
        .ok_or_else(|| anyhow::anyhow!("the unit process identity is unavailable"))?
        .1;
    // Field 22 (starttime), after the pid and parenthesized comm fields.
    let started = fields
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| anyhow::anyhow!("the unit process start identity is unavailable"))?;
    let namespace = std::fs::read_link("/proc/self/ns/pid")?;
    Ok(format!("{started}@{}", namespace.display()))
}

#[cfg(target_os = "linux")]
#[allow(
    unsafe_code,
    reason = "the one-shot helper uses authorized Linux namespace/device syscalls and a scalar KVM version query"
)]
mod linux {
    use super::*;
    use anyhow::Context;
    use nix::libc;
    use nix::sched::{CloneFlags, setns};
    use nix::sys::stat::{Mode, SFlag, major, makedev, minor, mknod};
    use nix::unistd::{Gid, Uid, chown, chroot};
    use rustix::fs::{Mode as FileMode, OFlags, ResolveFlags, mkdirat, open, openat2};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};

    // The query uses _IO but still requires a zero scalar argument. A two-argument
    // variadic call leaves an arbitrary register value which KVM refuses.
    nix::ioctl_write_int_bad!(kvm_api_version, nix::request_code_none!(0xAE, 0));

    pub(super) fn run(args: &Args) -> anyhow::Result<()> {
        if args.operation == Operation::KvmVersion {
            let device = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/kvm")?;
            // SAFETY: the borrowed live descriptor names this caller's device; this read-only
            // scalar query takes the required zero, with no pointer or persistent resource.
            let version = unsafe { kvm_api_version(device.as_raw_fd(), 0) }?;
            println!("{version}");
            return Ok(());
        }
        let source_is_file = args.operation == Operation::Attach
            && !std::fs::metadata("/src")
                .context("checking the Docker-bound source")?
                .is_dir();
        let source = if args.operation == Operation::Attach {
            let tree =
                clone_source().context("cloning the source mount requires Linux 5.12 or later")?;
            if args.read_only {
                readonly(&tree).context("making the source tree recursively read-only")?;
            }
            Some(tree)
        } else {
            None
        };
        let device = if args.operation == Operation::Device && !args.remove {
            let metadata =
                std::fs::symlink_metadata("/src").context("reading the bound host device")?;
            if !metadata.file_type().is_char_device() {
                anyhow::bail!("the source is not a character device");
            }
            let numbers = (major(metadata.rdev()), minor(metadata.rdev()));
            if Some(numbers.0) != args.major || Some(numbers.1) != args.minor {
                anyhow::bail!("the host device changed since its permission was prepared");
            }
            Some((numbers, metadata.gid(), metadata.mode() & 0o777))
        } else {
            None
        };
        let namespace = std::fs::File::open("/proc/1/ns/mnt")
            .context("opening the owned unit's mount namespace")?;
        let root = std::fs::File::open("/proc/1/root").context("opening the owned unit's root")?;
        setns(&namespace, CloneFlags::CLONE_NEWNS)
            .context("entering the owned unit's mount namespace")?;
        chroot(format!("/proc/self/fd/{}", root.as_raw_fd()).as_str())
            .context("using the owned unit's root")?;
        std::env::set_current_dir("/").context("entering the owned unit's root directory")?;
        match args.operation {
            Operation::Source
            | Operation::Identity
            | Operation::KvmVersion
            | Operation::Launch
            | Operation::DirectoryCheck
            | Operation::VolumeMeasure
            | Operation::VolumeFingerprint
            | Operation::VolumeExport
            | Operation::VolumeImport
            | Operation::FileStatesMeasure
            | Operation::FileStatesExport
            | Operation::FileStatesImport => {
                anyhow::bail!("this operation does not enter a unit namespace")
            }
            Operation::Attach => {
                let target = target(&args.target, args.file || source_is_file)?;
                check_open_target(&target, args)?;
                let tree = source.ok_or_else(|| anyhow::anyhow!("the source mount is missing"))?;
                move_tree(&tree, &target).context("moving the source mount onto the target")?;
            }
            Operation::Detach => {
                let root = open(
                    "/",
                    OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                    FileMode::empty(),
                )?;
                let target = match openat2(
                    &root,
                    args.target.trim_start_matches('/'),
                    OFlags::PATH | OFlags::CLOEXEC,
                    FileMode::empty(),
                    ResolveFlags::IN_ROOT | ResolveFlags::NO_MAGICLINKS,
                ) {
                    Ok(target) => target,
                    Err(rustix::io::Errno::NOENT) => return Ok(()),
                    Err(error) => return Err(error).context("opening the mount to detach"),
                };
                check_open_target(&target, args)?;
                // Follow this helper's own descriptor, not a target alias that another process can change.
                let pinned = format!("/proc/self/fd/{}", target.as_raw_fd());
                match nix::mount::umount2(pinned.as_str(), nix::mount::MntFlags::MNT_DETACH) {
                    Ok(()) | Err(nix::errno::Errno::EINVAL | nix::errno::Errno::ENOENT) => {}
                    Err(error) => return Err(error).context("detaching the mount"),
                }
            }
            Operation::Device if args.remove => match std::fs::remove_file(&args.target) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("removing the device node"),
            },
            Operation::Device => {
                let ((major, minor), group, mode) =
                    device.ok_or_else(|| anyhow::anyhow!("the host device is missing"))?;
                match std::fs::symlink_metadata(&args.target) {
                    Ok(metadata)
                        if metadata.file_type().is_char_device()
                            && metadata.rdev() == makedev(major, minor) => {}
                    Ok(_) => anyhow::bail!("the device target is already occupied"),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        mknod(
                            args.target.as_str(),
                            SFlag::S_IFCHR,
                            Mode::from_bits_truncate(mode),
                            makedev(major, minor),
                        )
                        .context("giving the device node")?;
                    }
                    Err(error) => return Err(error).context("checking the device target"),
                }
                chown(
                    args.target.as_str(),
                    Some(Uid::from_raw(0)),
                    Some(Gid::from_raw(group)),
                )
                .context("setting the host device's group")?;
                std::fs::set_permissions(&args.target, std::fs::Permissions::from_mode(mode))
                    .context("setting the host device's mode")?;
            }
        }
        Ok(())
    }

    #[repr(C)]
    struct MountAttr {
        attr_set: u64,
        attr_clr: u64,
        propagation: u64,
        userns_fd: u64,
    }

    fn syscall_result(result: libc::c_long) -> std::io::Result<libc::c_long> {
        if result < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(result)
        }
    }

    fn clone_source() -> std::io::Result<OwnedFd> {
        const OPEN_TREE_CLONE: libc::c_uint = 1;
        let flags =
            OPEN_TREE_CLONE | libc::O_CLOEXEC as libc::c_uint | libc::AT_RECURSIVE as libc::c_uint;
        // SAFETY: the constant source is NUL terminated, all flags are from the Linux ABI,
        // and a successful open_tree creates a new descriptor owned by this call.
        let result = syscall_result(unsafe {
            libc::syscall(libc::SYS_open_tree, libc::AT_FDCWD, c"/src".as_ptr(), flags)
        })?;
        // SAFETY: a nonnegative result is the newly allocated descriptor, never one borrowed from a caller.
        Ok(unsafe { OwnedFd::from_raw_fd(result as std::os::fd::RawFd) })
    }

    fn readonly(tree: &OwnedFd) -> std::io::Result<()> {
        const MOUNT_ATTR_RDONLY: u64 = 1;
        let attributes = MountAttr {
            attr_set: MOUNT_ATTR_RDONLY,
            attr_clr: 0,
            propagation: 0,
            userns_fd: 0,
        };
        // SAFETY: both borrowed descriptor and repr(C) attribute storage stay alive for the call;
        // the empty path applies attributes to that detached tree only, including its descendants.
        syscall_result(unsafe {
            libc::syscall(
                libc::SYS_mount_setattr,
                tree.as_raw_fd(),
                c"".as_ptr(),
                libc::AT_EMPTY_PATH | libc::AT_RECURSIVE,
                &attributes as *const MountAttr,
                std::mem::size_of::<MountAttr>(),
            )
        })?;
        Ok(())
    }

    fn move_tree(tree: &OwnedFd, target: &OwnedFd) -> std::io::Result<()> {
        const MOVE_MOUNT_F_EMPTY_PATH: libc::c_uint = 4;
        const MOVE_MOUNT_T_EMPTY_PATH: libc::c_uint = 0x40;
        // SAFETY: the two descriptors remain borrowed and live for this call. Empty paths select
        // their exact objects; no path traversal can redirect the destination after it was opened.
        syscall_result(unsafe {
            libc::syscall(
                libc::SYS_move_mount,
                tree.as_raw_fd(),
                c"".as_ptr(),
                target.as_raw_fd(),
                c"".as_ptr(),
                MOVE_MOUNT_F_EMPTY_PATH | MOVE_MOUNT_T_EMPTY_PATH,
            )
        })?;
        Ok(())
    }

    fn check_open_target(target: &OwnedFd, args: &Args) -> anyhow::Result<()> {
        let path = std::fs::read_link(format!("/proc/self/fd/{}", target.as_raw_fd()))
            .context("checking the resolved live target")?;
        let mut reserved = args.reserved.clone();
        reserved.extend(
            [
                "/run/orch",
                "/run/orch-capability-gate",
                "/opt/orch",
                "/var/lib/orch",
                "/proc",
                "/sys",
            ]
            .map(str::to_owned),
        );
        let roots: Vec<_> = reserved.iter().map(String::as_str).collect();
        let resolved = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("the live target is not UTF-8"))?;
        if !orch_driver_contract::mounts::check_targets(&[("live", resolved)], &roots).is_empty() {
            anyhow::bail!("the resolved target overlaps a protected root");
        }
        Ok(())
    }

    fn target(path: &str, file: bool) -> anyhow::Result<std::os::fd::OwnedFd> {
        let root = open(
            "/",
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            FileMode::empty(),
        )?;
        let components: Vec<_> = path
            .split('/')
            .filter(|component| !component.is_empty())
            .collect();
        let mut parent = open(
            "/",
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            FileMode::empty(),
        )?;
        let mut prefix = String::new();
        for (index, component) in components.iter().enumerate() {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            let last_file = file && index + 1 == components.len();
            let flags = OFlags::PATH
                | OFlags::CLOEXEC
                | if last_file {
                    OFlags::empty()
                } else {
                    OFlags::DIRECTORY
                };
            // Ordinary aliases such as /var/run stay inside the unit. The opened descriptor's
            // resolved path is checked against the actual protected roots before move_mount.
            let resolve = ResolveFlags::IN_ROOT | ResolveFlags::NO_MAGICLINKS;
            match openat2(&root, prefix.as_str(), flags, FileMode::empty(), resolve) {
                Ok(fd) => parent = fd,
                Err(rustix::io::Errno::NOENT) => {
                    if last_file {
                        let _ = rustix::fs::openat(
                            &parent,
                            *component,
                            OFlags::CREATE
                                | OFlags::EXCL
                                | OFlags::WRONLY
                                | OFlags::NOFOLLOW
                                | OFlags::CLOEXEC,
                            FileMode::from_bits_truncate(0o644),
                        )?;
                    } else {
                        match mkdirat(&parent, *component, FileMode::from_bits_truncate(0o755)) {
                            Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                            Err(error) => return Err(error.into()),
                        }
                    }
                    parent = openat2(&root, prefix.as_str(), flags, FileMode::empty(), resolve)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        // The open target is the mountpoint; a renamed path or a symlink cannot redirect the move.
        Ok(parent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_targets_reject_traversal_and_devices_outside_one_dev_component() {
        for target in ["../elsewhere", "/dev/../etc", "/dev/a/b", "/etc/kvm"] {
            let args = Args {
                operation: Operation::Device,
                target: target.into(),
                read_only: false,
                file: false,
                remove: true,
                major: None,
                minor: None,
                reserved: Vec::new(),
            };
            assert!(run(&args).is_err());
        }
    }
}
