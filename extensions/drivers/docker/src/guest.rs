//! What a unit under a runtime with a kernel of its own needs from its host: a resolver file and, for Docker inside, its data place.
//!
//! The daemon answers a unit's name lookups at 127.0.0.11 from inside the unit's own network namespace, which a
//! guest with a kernel of its own does not share, so such a unit is given a resolver file of its own: the filtered
//! resolver of its proxy when it is restricted, else the name servers the operator set or the host's own. Docker
//! inside keeps its data in a file of the state volume that is made into a file system and mounted at
//! `/var/lib/docker`, because the shared file system of a virtual machine cannot hold what an overlay is built
//! on. Both are made by the driver and nobody else.

use std::net::IpAddr;
use std::path::Path;

use bollard::models::ExecConfig;
use futures::StreamExt;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::isolation::Boundary;

use crate::client::{Daemon, map_error};
use crate::config::DockerConfig;
use crate::spec::layout;

/// Where the daemon's own resolver answers inside a unit that shares the host's network namespace view.
const EMBEDDED_RESOLVER: &str = "127.0.0.11";

/// The host's resolver file, and the file systemd's resolver keeps its real servers in where the first names only its
/// stub on loopback.
const HOST_RESOLV_CONF: &str = "/etc/resolv.conf";
const SYSTEMD_RESOLV_CONF: &str = "/run/systemd/resolve/resolv.conf";

/// What answers names when the host knows none the unit can reach: two public resolvers, the ones the daemon itself
/// falls back on.
pub const FALLBACK_RESOLVERS: [&str; 2] = ["1.1.1.1", "8.8.8.8"];

/// The name of the file in the run volume that holds a guest's resolver file, which is mounted over `/etc/resolv.conf`.
pub const RESOLV_FILE: &str = "resolv.conf";

/// Where a unit's resolver file is mounted.
pub const RESOLV_TARGET: &str = "/etc/resolv.conf";

/// The addresses a resolver file names that a unit elsewhere can use: not loopback, which would be the host itself.
fn usable_resolvers(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            (words.next() == Some("nameserver"))
                .then(|| words.next())
                .flatten()
        })
        .filter(|address| {
            address != &EMBEDDED_RESOLVER
                && address
                    .parse::<IpAddr>()
                    .is_ok_and(|parsed| !parsed.is_loopback() && !parsed.is_unspecified())
        })
        .map(str::to_owned)
        .collect()
}

/// The name servers of an open unit under such a runtime: what the operator set, else the host's own (from the
/// resolver file, or systemd's where that names only loopback), else the fallback.
#[must_use]
pub fn open_resolvers(config: &DockerConfig, template_dns: &[String]) -> Vec<String> {
    if !template_dns.is_empty() {
        return template_dns.to_vec();
    }
    if !config.resolvers.is_empty() {
        return config.resolvers.clone();
    }
    for file in [HOST_RESOLV_CONF, SYSTEMD_RESOLV_CONF] {
        if let Ok(text) = std::fs::read_to_string(Path::new(file)) {
            let found = usable_resolvers(&text);
            if !found.is_empty() {
                return found;
            }
        }
    }
    FALLBACK_RESOLVERS
        .iter()
        .map(|address| (*address).to_owned())
        .collect()
}

/// The text of a unit's resolver file.
#[must_use]
pub fn resolv_conf(nameservers: &[String]) -> String {
    let mut text = String::from(
        "# Made by Orch: this unit's runtime cannot reach the daemon's own resolver.\n",
    );
    for nameserver in nameservers {
        text.push_str("nameserver ");
        text.push_str(nameserver);
        text.push('\n');
    }
    text.push_str("options ndots:0\n");
    text
}

/// How Docker's data place is made in a unit that holds Docker inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataPlace {
    /// A file of the state volume made into a file system of its own and loop-mounted: for a virtual machine, whose
    /// shared file system cannot hold what an overlay is built on.
    LoopFile,
    /// A directory of the state volume mounted over Docker's data directory: for a kernel written in user space, which
    /// has no loop devices but runs an overlay on the volume as it is (measured under gVisor).
    Directory,
}

impl DataPlace {
    /// The place under a runtime's boundary.
    #[must_use]
    pub const fn under(boundary: Boundary) -> Self {
        match boundary {
            Boundary::UserSpaceKernel => Self::Directory,
            _ => Self::LoopFile,
        }
    }

    /// The word the preparing program is given.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::LoopFile => "loop",
            Self::Directory => "directory",
        }
    }
}

/// The program that prepares Docker inside, run as root in the unit each time it starts, given the state volume's
/// directory, the size in gigabytes and the kind of place ([`DataPlace`]). It makes the control groups ready for
/// Docker's containers; then it makes Docker's data directory a mount of the state volume: a directory of it, or a
/// file in it (made the first time, sparse so that it takes what its contents take, with a file system put in it
/// and loop-mounted; the nodes loop devices need are made when the unit has none). It does nothing where it is
/// already mounted. Exit status 5 says the image has no `mkfs.ext4`.
pub const PREPARE_DOCKER_INSIDE: &str = r#"set -eu
# Docker's own containers need the control groups writable, and their controllers handed down to children, which a
# container is not given: the processes already here move to a group of their own, so that the controllers can be.
# Keep the original mount root: Kata's later execs can have a cgroup namespace rooted at init's new group.
# Replacing that view hides the existing init group and makes another preparation move init again. The connector's
# children retain its original namespace, in which the delegated Docker subtree and its controllers are visible.
if [ -f /sys/fs/cgroup/cgroup.controllers ]; then
  mount -o remount,rw /sys/fs/cgroup 2>/dev/null || true
  if [ ! -d /sys/fs/cgroup/init ]; then
    mkdir -p /sys/fs/cgroup/init
    xargs -rn1 < /sys/fs/cgroup/cgroup.procs > /sys/fs/cgroup/init/cgroup.procs 2>/dev/null || true
  fi
  sed -e 's/ / +/g' -e 's/^/+/' < /sys/fs/cgroup/cgroup.controllers > /sys/fs/cgroup/cgroup.subtree_control 2>/dev/null || true
fi
# Process state belongs to this boot. Keeping old shim sockets and network namespaces in the unit's layer makes
# Docker try to reconnect to processes of the previous guest; images and containers belong in the state volume.
mkdir -p /var/run/docker
if ! mountpoint -q /var/run/docker 2>/dev/null; then
  mount -t tmpfs -o mode=0755 tmpfs /var/run/docker
fi
data=/var/lib/docker
file="$1/docker-inside.ext4"
gigabytes="$2"
place="${3:-loop}"
mkdir -p "$data"
if mountpoint -q "$data" 2>/dev/null; then exit 0; fi
if [ "$place" = directory ]; then
  mkdir -p "$1/docker-inside"
  mount --bind "$1/docker-inside" "$data"
  exit 0
fi
if [ ! -f "$file" ]; then
  command -v mkfs.ext4 >/dev/null 2>&1 || { echo "this image has no mkfs.ext4 (e2fsprogs)" >&2; exit 5; }
  truncate -s "${gigabytes}G" "$file"
  mkfs.ext4 -q -F -m 0 "$file"
fi
[ -e /dev/loop-control ] || mknod /dev/loop-control c 10 237
i=0
while [ "$i" -lt 8 ]; do [ -e "/dev/loop$i" ] || mknod "/dev/loop$i" b 7 "$i"; i=$((i + 1)); done
mount -o loop,noatime "$file" "$data"
"#;

/// What running a program as root in a unit came to.
#[derive(Debug)]
pub struct Ran {
    /// The program's exit status.
    pub status: i64,
    /// What it printed, both streams, cut to [`OUTPUT_BYTES`].
    pub output: String,
}

/// How long the place for Docker's data may take to be made before the start of the unit is given up: making a sparse
/// file and a file system in it, or mounting a directory, takes seconds; a hung mount must not hold the start for good.
const PREPARE_WAIT: std::time::Duration = std::time::Duration::from_secs(300);

/// How much of a program's output is kept, in bytes: enough for a reason, not a log.
const OUTPUT_BYTES: usize = 4_096;

/// Runs `sh -c script sh args…` as root in the running container `container`, and waits for it to end.
pub async fn run_as_root(
    daemon: &Daemon,
    container: &str,
    script: &str,
    args: &[&str],
) -> Result<Ran, DriverError> {
    let mut cmd = vec![
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        script.to_owned(),
        "orch".to_owned(),
    ];
    cmd.extend(args.iter().map(|argument| (*argument).to_owned()));
    let created = daemon
        .docker()
        .create_exec(
            container,
            ExecConfig {
                attach_stdout: Some(true),
                attach_stderr: Some(true),
                user: Some("0".to_owned()),
                cmd: Some(cmd),
                ..ExecConfig::default()
            },
        )
        .await
        .map_err(map_error)?;
    let started = daemon
        .docker()
        .start_exec(&created.id, None)
        .await
        .map_err(map_error)?;
    let mut output = String::new();
    if let bollard::exec::StartExecResults::Attached {
        output: mut stream, ..
    } = started
    {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(map_error)?;
            if output.len() < OUTPUT_BYTES {
                output.push_str(&String::from_utf8_lossy(&chunk.into_bytes()));
            }
        }
    }
    output.truncate(
        (0..=OUTPUT_BYTES.min(output.len()))
            .rev()
            .find(|cut| output.is_char_boundary(*cut))
            .unwrap_or(0),
    );
    let inspected = daemon
        .docker()
        .inspect_exec(&created.id)
        .await
        .map_err(map_error)?;
    Ok(Ran {
        status: inspected.exit_code.unwrap_or(-1),
        output,
    })
}

/// Prepares Docker inside in a running unit: the data file of `gigabytes` in its state volume, made a file system and
/// mounted where Docker keeps its data.
pub async fn prepare_docker_inside(
    daemon: &Daemon,
    container: &str,
    gigabytes: u64,
    place: DataPlace,
) -> Result<(), DriverError> {
    let size = gigabytes.to_string();
    let ran = tokio::time::timeout(
        PREPARE_WAIT,
        run_as_root(
            daemon,
            container,
            PREPARE_DOCKER_INSIDE,
            &[layout::STATE_DIR, &size, place.word()],
        ),
    )
    .await
    .map_err(|_| {
        DriverError::Unavailable(format!(
            "docker_inside: the place for Docker's data was not ready after {} seconds",
            PREPARE_WAIT.as_secs()
        ))
    })??;
    if ran.status == 0 {
        return Ok(());
    }
    Err(DriverError::Unavailable(format!(
        "docker_inside: the place for Docker's data could not be prepared (status {}): {}",
        ran.status,
        ran.output.trim()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hosts_resolver_file_gives_the_servers_a_unit_elsewhere_can_use() {
        let host = "# a comment\nnameserver 127.0.0.53\nnameserver 10.1.0.2\nnameserver ::1\nnameserver 2001:4860:4860::8888\nsearch example.org\nnameserver 127.0.0.11\n";
        assert_eq!(usable_resolvers(host), ["10.1.0.2", "2001:4860:4860::8888"]);
        assert!(usable_resolvers("nameserver 127.0.0.53\n").is_empty());
    }

    #[test]
    fn the_template_then_the_operator_then_the_host_decide_the_name_servers_of_an_open_unit() {
        let mut config = DockerConfig::default();
        assert_eq!(
            open_resolvers(&config, &["9.9.9.9".to_owned()]),
            ["9.9.9.9"]
        );
        config.resolvers = vec!["10.0.0.53".to_owned()];
        assert_eq!(open_resolvers(&config, &[]), ["10.0.0.53"]);
        assert_eq!(
            open_resolvers(&config, &["9.9.9.9".to_owned()]),
            ["9.9.9.9"]
        );
    }

    #[test]
    fn a_resolver_file_names_its_servers_and_says_why_it_exists() {
        let text = resolv_conf(&["10.199.4.18".to_owned()]);
        assert!(text.contains("nameserver 10.199.4.18\n"));
        assert!(text.starts_with('#'));
    }

    #[test]
    fn the_program_that_prepares_docker_inside_is_idempotent_and_says_what_an_image_lacks() {
        assert!(PREPARE_DOCKER_INSIDE.contains("mountpoint -q"));
        assert!(PREPARE_DOCKER_INSIDE.contains("exit 5"));
        assert!(PREPARE_DOCKER_INSIDE.contains("truncate -s"));
    }

    #[test]
    fn a_kernel_in_user_space_gets_a_directory_of_the_volume_and_a_virtual_machine_a_file_system_of_its_own()
     {
        assert_eq!(
            DataPlace::under(Boundary::UserSpaceKernel),
            DataPlace::Directory
        );
        assert_eq!(DataPlace::under(Boundary::OwnKernel), DataPlace::LoopFile);
        assert_eq!(DataPlace::Directory.word(), "directory");
        assert_eq!(DataPlace::LoopFile.word(), "loop");
        // The directory comes before anything is made a file system, and ends the program: no loop device is needed.
        let directory = PREPARE_DOCKER_INSIDE.find("mount --bind").unwrap();
        assert!(directory < PREPARE_DOCKER_INSIDE.find("mkfs.ext4 -q").unwrap());
        assert!(directory < PREPARE_DOCKER_INSIDE.find("mount -o loop").unwrap());
        assert!(PREPARE_DOCKER_INSIDE.contains(r#"place="${3:-loop}""#));
    }
}
