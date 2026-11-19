//! Journalled argv commands after a managed unit starts, bounded output and whole-group cancellation.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use orch_link_contract::commands::{CommandError, ErrorCode, UnitAfterUp, UnitAfterUpResult};
use orch_plugin_contract::secret::REDACTED;
use orch_types::Hash;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt};

use crate::credentials::SecretStore;
use crate::roles::Services;

const PREFIX: &str = "after-up-";
/// The design's limit is per stream; readers continue draining after this many bytes.
const OUTPUT_BYTES: usize = 16 * 1024;
/// A successful command may intentionally leave a helper with its output pipe open.
const DRAIN_GRACE: Duration = Duration::from_secs(1);

#[derive(Serialize, Deserialize)]
struct Kept {
    fingerprint: Hash,
    result: Option<UnitAfterUpResult>,
}

/// A role's serial writer. Intent is sealed and synced before spawn; results survive a reconnect.
pub(crate) struct Runner {
    store: Option<Arc<SecretStore>>,
    serial: tokio::sync::Mutex<()>,
}

fn unavailable() -> CommandError {
    CommandError::new(
        ErrorCode::Internal,
        "the after-start command journal is unavailable",
    )
}

impl Runner {
    pub(crate) fn new(services: &Services) -> Self {
        Self {
            store: SecretStore::open(&services.data_dir, Arc::clone(&services.keys))
                .ok()
                .map(Arc::new),
            serial: tokio::sync::Mutex::new(()),
        }
    }

    fn save(&self, name: &str, kept: &Kept) -> Result<(), CommandError> {
        let text = serde_json::to_string(kept).map_err(|_| unavailable())?;
        self.store
            .as_deref()
            .ok_or_else(unavailable)?
            .put(name, &text)
            .map_err(|_| unavailable())
    }

    pub(crate) async fn run(
        &self,
        body: &UnitAfterUp,
        cwd: &Path,
        variables: &BTreeMap<String, String>,
    ) -> Result<UnitAfterUpResult, CommandError> {
        self.run_in(body, cwd, variables, &[]).await
    }

    pub(crate) async fn run_in(
        &self,
        body: &UnitAfterUp,
        cwd: &Path,
        variables: &BTreeMap<String, String>,
        launcher: &[String],
    ) -> Result<UnitAfterUpResult, CommandError> {
        if !cfg!(unix) {
            return Err(CommandError::new(
                ErrorCode::Unsupported,
                "this runtime cannot enforce an after-start command's process group",
            ));
        }
        body.run
            .validate()
            .map_err(|message| CommandError::new(ErrorCode::Conflict, message))?;
        if body.environment_id.is_some() == body.provider_id.is_some()
            || body.name.trim().is_empty()
            || body.name.chars().any(char::is_control)
        {
            return Err(CommandError::new(
                ErrorCode::Conflict,
                "a command needs one environment or provider and a nonempty name without control characters",
            ));
        }
        let _serial = self.serial.lock().await;
        let store = self.store.as_deref().ok_or_else(unavailable)?;
        let unit = body
            .environment_id
            .or(body.provider_id)
            .ok_or_else(unavailable)?;
        let name = format!("{PREFIX}{unit}-{}-{}", body.run_id, body.position);
        let fingerprint = Hash::of(&serde_json::to_vec(body).map_err(|_| unavailable())?);
        if let Some(text) = store.get(&name).map_err(|_| unavailable())? {
            let mut kept: Kept = serde_json::from_str(&text).map_err(|_| unavailable())?;
            if kept.fingerprint != fingerprint {
                return Err(CommandError::new(
                    ErrorCode::Conflict,
                    "the commands for this unit run changed; start a new run",
                ));
            }
            if let Some(result) = kept.result {
                return Ok(result);
            }
            // A crash may have happened after spawn. Repeating the side effect is never safe.
            let result = UnitAfterUpResult { name: body.name.clone(), exit: None, duration_ms: 0, timed_out: false,
                first_lines: "the connector stopped before this command's result was saved; start a new unit run".into() };
            kept.result = Some(result.clone());
            self.save(&name, &kept)?;
            return Ok(result);
        }
        // Collect known secrets before executing, and never put raw output in a journal or log.
        let mut secrets: Vec<String> = std::env::vars_os()
            .filter_map(|(_, value)| value.into_string().ok())
            .chain(variables.values().cloned())
            .filter(|value| !value.is_empty())
            .collect();
        for key in store.names().map_err(|_| unavailable())? {
            if key.starts_with(PREFIX) {
                continue;
            }
            if let Some(value) = store.get(&key).map_err(|_| unavailable())? {
                secret_values(&value, &mut secrets);
            }
        }
        secrets.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        secrets.dedup();
        self.save(
            &name,
            &Kept {
                fingerprint,
                result: None,
            },
        )?;
        let result = execute(body, cwd, variables, &secrets, launcher).await;
        self.save(
            &name,
            &Kept {
                fingerprint,
                result: Some(result.clone()),
            },
        )?;
        Ok(result)
    }
}

fn secret_values(text: &str, secrets: &mut Vec<String>) {
    fn leaves(value: &serde_json::Value, secrets: &mut Vec<String>) {
        match value {
            serde_json::Value::String(text) if !text.is_empty() => secrets.push(text.clone()),
            serde_json::Value::Array(values) => {
                for value in values {
                    leaves(value, secrets);
                }
            }
            serde_json::Value::Object(values) => {
                for value in values.values() {
                    leaves(value, secrets);
                }
            }
            _ => {}
        }
    }
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text) {
        leaves(&value, secrets);
    }
    if !text.is_empty() {
        secrets.push(text.to_owned());
    }
}

/// Retains the group id after wait() has cleared the child's id. Cancellation kills descendants too.
struct Group(Option<u32>);
impl Drop for Group {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self
            .0
            .and_then(|pid| i32::try_from(pid).ok())
            .and_then(rustix::process::Pid::from_raw)
        {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
}

type Output = Arc<Mutex<Vec<u8>>>;
async fn drain(mut stream: impl AsyncRead + Unpin, kept: Output) {
    let mut buffer = [0u8; 4096];
    while let Ok(size) = stream.read(&mut buffer).await {
        if size == 0 {
            break;
        }
        let mut bytes = kept.lock().unwrap_or_else(PoisonError::into_inner);
        let take = size.min(OUTPUT_BYTES.saturating_sub(bytes.len()));
        bytes.extend_from_slice(&buffer[..take]);
    }
}

async fn execute(
    body: &UnitAfterUp,
    cwd: &Path,
    variables: &BTreeMap<String, String>,
    secrets: &[String],
    launcher: &[String],
) -> UnitAfterUpResult {
    let began = tokio::time::Instant::now();
    let mut result = UnitAfterUpResult {
        name: body.name.clone(),
        exit: None,
        duration_ms: 0,
        timed_out: false,
        first_lines: String::new(),
    };
    let mut command = tokio::process::Command::from(orch_envhost::process_limits::command(
        &body.run.command[0],
        launcher,
    ));
    command
        .args(&body.run.command[1..])
        .current_dir(cwd)
        .envs(variables)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            result.first_lines = format!("the command could not run ({})", error.kind());
            return result;
        }
    };
    let mut group = Group(child.id());
    let stdout: Output = Arc::default();
    let stderr: Output = Arc::default();
    let out = child
        .stdout
        .take()
        .map(|stream| tokio::spawn(drain(stream, Arc::clone(&stdout))));
    let err = child
        .stderr
        .take()
        .map(|stream| tokio::spawn(drain(stream, Arc::clone(&stderr))));
    match tokio::time::timeout(
        Duration::from_secs(u64::from(body.run.timeout_secs)),
        child.wait(),
    )
    .await
    {
        Ok(Ok(status)) => result.exit = status.code(),
        Ok(Err(_)) => result.first_lines = "the command's exit could not be read".into(),
        Err(_) => result.timed_out = true,
    }
    if result.passed() {
        group.0 = None;
    } else {
        drop(group);
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    let collect = |task: Option<tokio::task::JoinHandle<()>>| async move {
        if let Some(task) = task {
            let abort = task.abort_handle();
            if tokio::time::timeout(DRAIN_GRACE, task).await.is_err() {
                abort.abort();
            }
        }
    };
    tokio::join!(collect(out), collect(err));
    if result.first_lines.is_empty() {
        let err = stderr.lock().unwrap_or_else(PoisonError::into_inner);
        let out = stdout.lock().unwrap_or_else(PoisonError::into_inner);
        result.first_lines = first_lines(
            if err.iter().any(|byte| !byte.is_ascii_whitespace()) {
                &err
            } else {
                &out
            },
            secrets,
        );
    }
    result.duration_ms = u64::try_from(began.elapsed().as_millis()).unwrap_or(u64::MAX);
    result
}

fn first_lines(bytes: &[u8], secrets: &[String]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let mut scrubbed = String::new();
    // A byte-bound stream can finish inside a UTF-8 character of a secret.
    let mut rest = text.trim_end_matches('\u{fffd}');
    while !rest.is_empty() {
        if let Some(secret) = secrets
            .iter()
            .find(|secret| !secret.is_empty() && rest.starts_with(secret.as_str()))
        {
            scrubbed.push_str(REDACTED);
            rest = &rest[secret.len()..];
        } else if secrets
            .iter()
            .any(|secret| !secret.is_empty() && secret.starts_with(rest))
        {
            // A stream truncated at its byte limit must not reveal a partial secret at its edge.
            scrubbed.push_str(REDACTED);
            break;
        } else if let Some(character) = rest.chars().next() {
            scrubbed.push(if character.is_control() && character != '\n' {
                '?'
            } else {
                character
            });
            rest = &rest[character.len_utf8()..];
        }
    }
    scrubbed
        .lines()
        .take(10)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .take(1000)
        .collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::roles::Keys;
    use orch_pki::certificates::LeafKey;
    use orch_settings_contract::unit::AfterUpCommand;
    use orch_types::new_id;

    fn runner(dir: &Path, keys: &Arc<Keys>) -> Result<Runner, crate::credentials::StoreError> {
        let store = SecretStore::open(dir, Arc::clone(keys))?;
        Ok(Runner {
            store: Some(Arc::new(store)),
            serial: tokio::sync::Mutex::new(()),
        })
    }
    fn command(argv: &[&str]) -> UnitAfterUp {
        UnitAfterUp {
            run_id: new_id(),
            environment_id: Some(new_id()),
            provider_id: None,
            agent_id: None,
            position: 0,
            name: "10-tools".into(),
            run: AfterUpCommand {
                command: argv.iter().map(|arg| (*arg).into()).collect(),
                timeout_secs: 1,
            },
        }
    }
    #[tokio::test]
    async fn replay_after_reopening_answers_without_running_and_new_run_runs_again() {
        let dir = tempfile::tempdir().unwrap();
        let keys = Arc::new(Keys::new(LeafKey::generate().unwrap()));
        let mut body = command(&[
            "/bin/sh",
            "-c",
            "printf '%s\\n' \"$1\" >> seen",
            "literal",
            "a b;$HOME",
        ]);
        let first = runner(dir.path(), &keys)
            .unwrap()
            .run(&body, dir.path(), &BTreeMap::new())
            .await
            .unwrap();
        assert!(first.passed());
        assert_eq!(
            runner(dir.path(), &keys)
                .unwrap()
                .run(&body, dir.path(), &BTreeMap::new())
                .await
                .unwrap(),
            first
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("seen")).unwrap(),
            "a b;$HOME\n"
        );
        body.run_id = new_id();
        assert!(
            runner(dir.path(), &keys)
                .unwrap()
                .run(&body, dir.path(), &BTreeMap::new())
                .await
                .unwrap()
                .passed()
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("seen"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }
    #[tokio::test]
    async fn failed_and_timed_out_groups_end_and_output_is_drained_bounded_and_scrubbed() {
        let dir = tempfile::tempdir().unwrap();
        let keys = Arc::new(Keys::new(LeafKey::generate().unwrap()));
        let variables = BTreeMap::from([("API_KEY".into(), "private-value-927".into())]);
        let body = command(&[
            "/bin/sh",
            "-c",
            "printf '%s\\n' \"$API_KEY\"; head -c 65536 /dev/zero; exit 7",
        ]);
        let result = runner(dir.path(), &keys)
            .unwrap()
            .run(&body, dir.path(), &variables)
            .await
            .unwrap();
        assert_eq!(result.exit, Some(7));
        assert!(result.first_lines.chars().count() <= 1000);
        assert!(!result.first_lines.contains("private-value-927"));
        let body = command(&["/bin/sh", "-c", "(sleep 2; echo escaped > escaped) & wait"]);
        let result = runner(dir.path(), &keys)
            .unwrap()
            .run(&body, dir.path(), &BTreeMap::new())
            .await
            .unwrap();
        assert!(result.timed_out);
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(!dir.path().join("escaped").exists());
    }
    #[tokio::test]
    async fn saved_intent_without_result_fails_closed_without_a_second_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let keys = Arc::new(Keys::new(LeafKey::generate().unwrap()));
        let runner = runner(dir.path(), &keys).unwrap();
        let body = command(&["/bin/sh", "-c", "touch repeated"]);
        let name = format!("{PREFIX}{}-{}-0", body.environment_id.unwrap(), body.run_id);
        runner
            .save(
                &name,
                &Kept {
                    fingerprint: Hash::of(&serde_json::to_vec(&body).unwrap()),
                    result: None,
                },
            )
            .unwrap();
        assert!(
            !runner
                .run(&body, dir.path(), &BTreeMap::new())
                .await
                .unwrap()
                .passed()
        );
        assert!(!dir.path().join("repeated").exists());
    }
    #[tokio::test]
    async fn cancellation_kills_descendants_and_the_saved_intent_never_executes_again() {
        let dir = tempfile::tempdir().unwrap();
        let keys = Arc::new(Keys::new(LeafKey::generate().unwrap()));
        let runner = Arc::new(runner(dir.path(), &keys).unwrap());
        let body = command(&[
            "/bin/sh",
            "-c",
            "echo began > began; (sleep 2; echo escaped > escaped) & wait",
        ]);
        let running = Arc::clone(&runner);
        let request = body.clone();
        let cwd = dir.path().to_path_buf();
        let task = tokio::spawn(async move { running.run(&request, &cwd, &BTreeMap::new()).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while !dir.path().join("began").exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        task.abort();
        let _ = task.await;
        let result = runner
            .run(&body, dir.path(), &BTreeMap::new())
            .await
            .unwrap();
        assert!(!result.passed());
        assert!(
            result
                .first_lines
                .contains("before this command's result was saved")
        );
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(!dir.path().join("escaped").exists());
    }
    #[tokio::test]
    async fn changing_argv_under_one_run_position_is_refused_and_spawn_failures_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let keys = Arc::new(Keys::new(LeafKey::generate().unwrap()));
        let runner = runner(dir.path(), &keys).unwrap();
        let mut body = command(&["/this/program/does/not/exist"]);
        let first = runner
            .run(&body, dir.path(), &BTreeMap::new())
            .await
            .unwrap();
        assert!(!first.passed());
        assert_eq!(first.exit, None);
        assert_eq!(
            runner
                .run(&body, dir.path(), &BTreeMap::new())
                .await
                .unwrap(),
            first
        );
        body.run.command = vec!["/bin/true".into()];
        assert!(
            runner
                .run(&body, dir.path(), &BTreeMap::new())
                .await
                .is_err()
        );
    }
    #[test]
    fn output_respects_line_character_and_partial_secret_bounds() {
        let secrets = vec!["long-secret-value".to_owned()];
        assert!(!first_lines(b"start\nlong-secret-value\nend", &secrets).contains("long-secret"));
        assert_eq!(
            first_lines(b"start long-secr", &secrets),
            format!("start {REDACTED}")
        );
        assert_eq!(
            first_lines(b"start secret-\xc3", &["secret-é-value".into()]),
            format!("start {REDACTED}")
        );
        let text = (0..30)
            .map(|_| "line".repeat(50))
            .collect::<Vec<_>>()
            .join("\n");
        let result = first_lines(text.as_bytes(), &[]);
        assert!(result.lines().count() <= 10 && result.chars().count() <= 1000);
    }
}
