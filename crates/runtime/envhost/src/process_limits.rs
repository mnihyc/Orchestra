//! Proved user resource controls and aggregate groups for environment and provider processes.

use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use orch_driver_contract::process_limits::{PROBES, ProcessControls};
use orch_driver_contract::runtime::Resources;
use orch_types::Id;

// A missing user manager or authorization service must not hold an attachment indefinitely.
const MANAGER_WAIT: Duration = Duration::from_secs(5);

async fn run(program: &str, args: &[String]) -> Result<std::process::Output, String> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(MANAGER_WAIT, command.output()).await {
        Ok(Ok(output)) if output.status.success() => Ok(output),
        Ok(Ok(_)) => Err("the user's resource manager refused this resource group".into()),
        Ok(Err(_)) => Err("the user's resource manager is unavailable".into()),
        Err(_) => Err("the user's resource manager did not answer in five seconds".into()),
    }
}

/// One aggregate slice: every owned process of the environment or provider shares its ceiling.
#[derive(Debug, Clone)]
pub struct UserGroup {
    slice: String,
    anchor: String,
    cgroup: PathBuf,
    launcher: Vec<String>,
}

impl UserGroup {
    async fn anchor(id: Id) -> Result<Self, String> {
        let slice = format!("orch-{}.slice", id.simple());
        let anchor = format!("orch-{}-anchor.service", id.simple());
        if run(
            "systemctl",
            &[
                "--user".into(),
                "is-active".into(),
                "--quiet".into(),
                anchor.clone(),
            ],
        )
        .await
        .is_err()
        {
            run(
                "systemd-run",
                &[
                    "--user".into(),
                    "--quiet".into(),
                    "--collect".into(),
                    "--remain-after-exit".into(),
                    "--property=Type=exec".into(),
                    format!("--unit={anchor}"),
                    format!("--slice={slice}"),
                    "/usr/bin/true".into(),
                ],
            )
            .await?;
        }
        let shown = run(
            "systemctl",
            &[
                "--user".into(),
                "show".into(),
                "--property=ControlGroup".into(),
                "--value".into(),
                slice.clone(),
            ],
        )
        .await?;
        let path = String::from_utf8(shown.stdout)
            .map_err(|_| "the resource manager returned an invalid group path".to_owned())?;
        let path = path.trim();
        if path.is_empty()
            || !path.starts_with('/')
            || Path::new(path)
                .components()
                .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)))
        {
            return Err("the resource manager returned an invalid group path".into());
        }
        let systemd = crate::browser_discovery::find_program("systemd-run")
            .ok_or_else(|| "the resource launcher is unavailable".to_owned())?;
        let mut launcher = vec!["/usr/bin/env".into()];
        for key in ["XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS"] {
            if let Ok(value) = std::env::var(key) {
                launcher.push(format!("{key}={value}"));
            }
        }
        launcher.extend([
            systemd.to_string_lossy().into_owned(),
            "--user".into(),
            "--scope".into(),
            "--quiet".into(),
            "--collect".into(),
            "--expand-environment=no".into(),
            format!("--slice={slice}"),
            "--".into(),
        ]);
        Ok(Self {
            slice,
            anchor,
            cgroup: Path::new("/sys/fs/cgroup").join(path.trim_start_matches('/')),
            launcher,
        })
    }

    async fn set(&self, properties: Vec<String>) -> Result<(), String> {
        if properties.is_empty() {
            return Ok(());
        }
        let mut args = vec![
            "--user".into(),
            "set-property".into(),
            "--runtime".into(),
            self.slice.clone(),
        ];
        args.extend(properties);
        run("systemctl", &args).await.map(|_| ())
    }

    /// Probe each actual kernel controller, rather than treating a scope as three proved limits.
    pub async fn probe() -> ProcessControls {
        if !cfg!(target_os = "linux") {
            return ProcessControls::default();
        }
        let Ok(group) = Self::anchor(orch_types::new_id()).await else {
            return ProcessControls::default();
        };
        let mut command = group.launcher();
        command.push("/usr/bin/true".into());
        if run(&command[0], &command[1..]).await.is_err() {
            group.close().await;
            return ProcessControls::default();
        }
        let mut proved = [false; 3];
        for (index, property) in PROBES.into_iter().enumerate() {
            if group.set(vec![property.into()]).await.is_ok() {
                let (file, expected) = match index {
                    0 => ("cpu.max", None),
                    1 => ("memory.max", Some("67108864")),
                    _ => ("pids.max", Some("64")),
                };
                let value = std::fs::read_to_string(group.cgroup.join(file)).unwrap_or_default();
                proved[index] = expected.map_or_else(
                    || {
                        let mut parts = value.split_whitespace();
                        let quota = parts.next().and_then(|word| word.parse::<u64>().ok());
                        let period = parts.next().and_then(|word| word.parse::<u64>().ok());
                        quota
                            .zip(period)
                            .is_some_and(|(quota, period)| period > 0 && quota == period)
                    },
                    |expected| value.trim() == expected,
                );
            }
        }
        group.close().await;
        ProcessControls {
            cpu: proved[0],
            memory: proved[1],
            pids: proved[2],
        }
    }

    /// An advertised controller that stops working fails the attachment; it never silently stops enforcing.
    pub async fn open(
        id: Id,
        resources: &Resources,
        controls: ProcessControls,
    ) -> Result<Option<Self>, String> {
        if [
            resources.cpu_millis,
            resources.memory_mb,
            resources.pids,
            resources.disk_gb,
        ]
        .into_iter()
        .flatten()
        .any(|value| value <= 0)
        {
            return Err("resource ceilings must be positive".into());
        }
        if !controls.any() {
            return Ok(None);
        }
        if !(controls.cpu && resources.cpu_millis.is_some()
            || controls.memory && resources.memory_mb.is_some()
            || controls.pids && resources.pids.is_some())
        {
            return Ok(None);
        }
        let group = Self::anchor(id).await?;
        if let Err(error) = group.update(resources, controls).await {
            group.close().await;
            return Err(error);
        }
        Ok(Some(group))
    }

    /// Updating an existing group also removes an omitted ceiling; its existing children stay inside it.
    pub async fn update(
        &self,
        resources: &Resources,
        controls: ProcessControls,
    ) -> Result<(), String> {
        self.set(controls.properties(resources, true)).await?;
        // The user manager can leave an unlimited controller inactive. Require its kernel
        // file only when this group actually requests that proved controller's ceiling.
        for (required, file) in [
            (controls.cpu && resources.cpu_millis.is_some(), "cpu.max"),
            (
                controls.memory && resources.memory_mb.is_some(),
                "memory.max",
            ),
            (controls.pids && resources.pids.is_some(), "pids.max"),
        ] {
            if required && !self.cgroup.join(file).is_file() {
                return Err(format!(
                    "the previously proved resource control {file} is unavailable"
                ));
            }
        }
        Ok(())
    }

    /// Scope creation preserves the caller's pipes, terminal, working directory and environment.
    #[must_use]
    pub fn launcher(&self) -> Vec<String> {
        self.launcher.clone()
    }

    /// The aggregate's kernel counters, also used for its live resource sample.
    #[must_use]
    pub fn cgroup(&self) -> &Path {
        &self.cgroup
    }

    /// Stop only the group belonging to this environment or provider; recovery otherwise keeps it.
    pub async fn close(&self) {
        let _ = run(
            "systemctl",
            &[
                "--user".into(),
                "stop".into(),
                self.anchor.clone(),
                self.slice.clone(),
            ],
        )
        .await;
    }

    /// Adapters accept a program path; an owned wrapper keeps their children in the same aggregate.
    pub fn program(&self, directory: &Path, program: &str) -> Result<String, String> {
        let quote = |word: &str| format!("'{}'", word.replace('\'', "'\\''"));
        let mut argv = self.launcher();
        argv.push(program.into());
        let text = format!(
            "#!/bin/sh\nexec {} \"$@\"\n",
            argv.iter()
                .map(|word| quote(word))
                .collect::<Vec<_>>()
                .join(" ")
        );
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        let file = directory.join(format!(
            "program-{}.sh",
            orch_types::Hash::of(program.as_bytes())
        ));
        let temporary = directory.join(format!(".program-{}.tmp", orch_types::new_id().simple()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o700);
        }
        use std::io::Write;
        options
            .open(&temporary)
            .and_then(|mut file| file.write_all(text.as_bytes()))
            .map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &file).map_err(|error| error.to_string())?;
        Ok(file.to_string_lossy().into_owned())
    }
}

/// Prefix a process before it starts; joining an already running process would leave an unbounded race.
#[must_use]
pub fn command(program: impl AsRef<std::ffi::OsStr>, launcher: &[String]) -> std::process::Command {
    if let Some((wrapper, args)) = launcher.split_first() {
        let mut command = std::process::Command::new(wrapper);
        command.args(args).arg(program);
        command
    } else {
        std::process::Command::new(program)
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        reason = "a test fails by panicking"
    )]
    use super::*;
    use orch_driver_contract::descriptor::ResourceLimits;

    #[test]
    fn a_prefix_keeps_the_program_and_every_argument_as_words() {
        let launcher = vec![
            "native-launcher".into(),
            "--group=one environment".into(),
            "--".into(),
        ];
        let mut child = command("/a program", &launcher);
        child.arg("$(must remain data)");
        assert_eq!(child.get_program(), "native-launcher");
        assert_eq!(
            child.get_args().collect::<Vec<_>>(),
            vec![
                "--group=one environment",
                "--",
                "/a program",
                "$(must remain data)"
            ]
        );
    }

    #[tokio::test]
    async fn a_user_group_reports_actual_support_and_holds_its_started_commands_to_one_kernel_ceiling()
     {
        let controls = UserGroup::probe().await;
        let support = controls.support();
        assert_eq!(support.disk, ResourceLimits::Reported);
        if !controls.memory {
            assert_eq!(support.memory, ResourceLimits::Reported);
            return;
        }
        let id = orch_types::new_id();
        let resources = Resources {
            memory_mb: Some(64),
            ..Default::default()
        };
        let group = UserGroup::open(id, &resources, controls)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(group.cgroup().join("memory.max"))
                .unwrap()
                .trim(),
            "67108864"
        );
        let launcher = group.launcher();
        for _ in 0..2 {
            let mut child = tokio::process::Command::from(command("/bin/cat", &launcher));
            child.arg("/proc/self/cgroup");
            let output = tokio::time::timeout(MANAGER_WAIT, child.output())
                .await
                .unwrap()
                .unwrap();
            assert!(output.status.success());
            assert!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .contains(&format!("orch-{}.slice", id.simple())),
                "separate commands share the aggregate"
            );
        }
        // The allocation is twice the advertised memory ceiling; a successful print would prove a bypass.
        let mut child = tokio::process::Command::from(command("python3", &launcher));
        child.args([
            "-I",
            "-B",
            "-c",
            "value=bytearray(128*1024*1024); print('escaped')",
        ]);
        let output = tokio::time::timeout(MANAGER_WAIT, child.output())
            .await
            .unwrap()
            .unwrap();
        assert!(!output.status.success());
        assert!(!String::from_utf8_lossy(&output.stdout).contains("escaped"));
        group.update(&Resources::default(), controls).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(group.cgroup().join("memory.max"))
                .unwrap()
                .trim(),
            "max"
        );
        group.close().await;
    }
}
