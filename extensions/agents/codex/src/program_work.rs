//! One native process uses only disposable unit files and streams; connector credentials stay outside.
use crate::process::{
    HarnessProcess, LaunchError, LaunchRequest, Launcher, ProcessControl, ProcessExit,
};
use async_trait::async_trait;
use orch_agent_contract::program_work::{
    ProgramIo, WorkFile, WorkOutputStream, WorkProcess, WorkProcessSpec,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub(super) struct Context {
    pub identity: crate::model_catalogue::Identity,
    pub io: Arc<dyn ProgramIo>,
    _staging: Arc<tempfile::TempDir>,
}

impl crate::CodexAdapter {
    pub(super) async fn open_unit(
        &mut self,
        spec: orch_agent_contract::session::SessionSpec,
        io: Arc<dyn ProgramIo>,
    ) -> Result<
        orch_agent_contract::session::SessionOpened,
        orch_agent_contract::adapter::AdapterError,
    > {
        use orch_agent_contract::{
            adapter::{AdapterError, AgentAdapter},
            session::{ProviderAccess, SessionMode},
        };
        let thread = spec.settings.thread_id;
        if !matches!(spec.provider, ProviderAccess::Native { .. })
            || spec.settings.mode != SessionMode::New
            || !spec.settings.orch_tools.is_empty()
            || !spec.mcp.is_empty()
            || spec.skills.is_some()
            || spec.settings.memory.is_some()
            || !spec.settings.instructions.is_empty()
        {
            return Err(AdapterError::Unsupported(
                "isolated work accepts only its new frozen native request".into(),
            ));
        }
        if self.unit_work.contains_key(&thread) {
            return self.open_session(spec).await;
        }
        let (_, normal) = self.live()?;
        let home = normal.home.as_ref().ok_or(AdapterError::LoginRequired)?;
        let auth = std::fs::read(home.join(crate::process::AUTH_FILE))
            .map_err(|_| AdapterError::LoginRequired)?;
        let staging = Arc::new(
            tempfile::tempdir()
                .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?,
        );
        std::fs::write(staging.path().join(crate::process::AUTH_FILE), auth)
            .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;
        let mut start = normal.start.clone();
        start.home = Some(staging.path().to_string_lossy().into_owned());
        start.profile.attach = None;
        start.profile.env.clear();
        // Place instructions, MCP, memory and plugins are absent in this dedicated capability home.
        start.profile.config = serde_json::json!({});
        let events = normal.events.clone();
        let provider = start.provider_id;
        let identity = crate::model_catalogue::Identity {
            provider,
            fingerprint: orch_types::Hash::of(thread.as_bytes()),
        };
        let mut isolated = Self::with_launcher(Arc::new(UnitLauncher {
            io: io.clone(),
            staging: staging.clone(),
        }))
        .with_clock(self.clock.clone())
        .with_patience(self.patience);
        isolated.native_schemas = self.native_schemas.clone();
        let running = isolated
            .launch_model(start, events, None, Some(identity))
            .await?;
        self.model_processes.insert(identity, running);
        self.unit_work.insert(
            thread,
            Context {
                identity,
                io: io.clone(),
                _staging: staging,
            },
        );
        match self.open_session(spec).await {
            Ok(opened) => Ok(opened),
            Err(error) => {
                self.unit_work.remove(&thread);
                if let Some(running) = self.model_processes.remove(&identity) {
                    Self::stop_running(running).await;
                }
                let _ = io.retire().await;
                Err(error)
            }
        }
    }
}

pub(super) struct UnitLauncher {
    pub io: Arc<dyn ProgramIo>,
    pub staging: Arc<tempfile::TempDir>,
}
fn error(program: &str, message: impl std::fmt::Display) -> LaunchError {
    LaunchError::Spawn {
        program: program.to_owned(),
        source: std::io::Error::other(message.to_string()),
    }
}
fn local_home(home: &std::path::Path) -> String {
    home.to_string_lossy().into_owned()
}
fn mapped(value: &str, home: &std::path::Path) -> String {
    value.replace(&local_home(home), "/work/home")
}

#[async_trait]
impl Launcher for UnitLauncher {
    async fn launch(&self, request: LaunchRequest) -> Result<HarnessProcess, LaunchError> {
        let home = self.staging.path();
        let mut files = Vec::new();
        for name in [crate::process::AUTH_FILE, crate::process::CONFIG_FILE] {
            let bytes =
                std::fs::read(home.join(name)).map_err(|cause| error(&request.program, cause))?;
            let bytes = if name == crate::process::CONFIG_FILE {
                mapped(
                    &String::from_utf8(bytes).map_err(|cause| error(&request.program, cause))?,
                    home,
                )
                .into_bytes()
            } else {
                bytes
            };
            files.push(WorkFile {
                path: format!("home/{name}"),
                bytes,
            });
        }
        let daemon = PathBuf::from(crate::process::DAEMON_DIRECTORY)
            .join(crate::process::DAEMON_SETTINGS_FILE);
        if let Ok(bytes) = std::fs::read(home.join(&daemon)) {
            files.push(WorkFile {
                path: format!("home/{}", daemon.display()),
                bytes,
            });
        }
        self.io
            .prepare(files)
            .await
            .map_err(|cause| error(&request.program, cause))?;
        let env = BTreeMap::from([(crate::process::HOME_VARIABLE.into(), "/work/home".into())]);
        let process = self
            .io
            .launch(WorkProcessSpec {
                program: request.program.clone(),
                args: request
                    .args
                    .iter()
                    .map(|value| mapped(value, home))
                    .collect(),
                env,
                cwd: None,
            })
            .await
            .map_err(|cause| error(&request.program, cause))?;
        Ok(streams(process))
    }
}

/// These bounded pipes are IO chunks; native output and requests continue until their normal end.
fn streams(process: Arc<dyn WorkProcess>) -> HarnessProcess {
    let (stdin, mut input) = tokio::io::duplex(crate::process::STDERR_READ_BYTES);
    let (mut output, stdout) = tokio::io::duplex(crate::process::STDERR_READ_BYTES);
    let (mut diagnostics, stderr) = tokio::io::duplex(crate::process::STDERR_READ_BYTES);
    let writer = process.clone();
    tokio::spawn(async move {
        let mut bytes = vec![0; crate::process::STDERR_READ_BYTES];
        loop {
            match input.read(&mut bytes).await {
                Ok(0) => {
                    let _ = writer.close_input().await;
                    break;
                }
                Ok(count) => {
                    if writer.write(&bytes[..count]).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let reader = process.clone();
    tokio::spawn(async move {
        while let Ok(Some(chunk)) = reader.next_output().await {
            let result = match chunk.stream {
                WorkOutputStream::Stdout => output.write_all(&chunk.bytes).await,
                WorkOutputStream::Stderr => diagnostics.write_all(&chunk.bytes).await,
            };
            if result.is_err() {
                break;
            }
        }
    });
    HarnessProcess {
        stdin: Box::new(stdin),
        stdout: Box::new(stdout),
        stderr: Some(Box::new(stderr)),
        control: Box::new(Control(process)),
    }
}
struct Control(Arc<dyn WorkProcess>);
#[async_trait]
impl ProcessControl for Control {
    async fn wait(&mut self) -> ProcessExit {
        match self.0.wait().await {
            Ok(code) => ProcessExit {
                code: code.and_then(|code| i32::try_from(code).ok()),
                detail: String::new(),
            },
            Err(error) => ProcessExit {
                code: None,
                detail: error.to_string(),
            },
        }
    }
    fn kill(&mut self) {
        let process = self.0.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = process.terminate().await;
            });
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a case fails by panicking")]
mod tests {
    use super::*;
    use orch_agent_contract::{adapter::AdapterError, program_work::WorkProcess};
    use std::sync::Mutex;
    #[derive(Default)]
    struct Capture {
        files: Mutex<Vec<WorkFile>>,
        process: Mutex<Option<WorkProcessSpec>>,
    }
    #[async_trait]
    impl ProgramIo for Capture {
        async fn prepare(&self, files: Vec<WorkFile>) -> Result<(), AdapterError> {
            *self.files.lock().unwrap() = files;
            Ok(())
        }
        async fn launch(
            &self,
            process: WorkProcessSpec,
        ) -> Result<Arc<dyn WorkProcess>, AdapterError> {
            *self.process.lock().unwrap() = Some(process);
            Err(AdapterError::Unsupported(
                "the case stops before native launch".into(),
            ))
        }
        async fn read_file(&self, _: &str, _: u64) -> Result<Vec<u8>, AdapterError> {
            Err(AdapterError::Unsupported("not needed".into()))
        }
        async fn retire(&self) -> Result<(), AdapterError> {
            Ok(())
        }
    }
    #[tokio::test]
    async fn an_isolated_launch_receives_only_its_selected_signin_and_fresh_options_without_connector_credentials()
     {
        let staging = Arc::new(tempfile::tempdir().unwrap());
        std::fs::write(
            staging.path().join(crate::process::AUTH_FILE),
            b"synthetic-selected-signin",
        )
        .unwrap();
        std::fs::write(
            staging.path().join(crate::process::CONFIG_FILE),
            format!("history_path = {:?}\n", staging.path().join("history")),
        )
        .unwrap();
        std::fs::write(
            staging.path().join("another-person-note"),
            b"must not be copied",
        )
        .unwrap();
        let capture = Arc::new(Capture::default());
        let launcher = UnitLauncher {
            io: capture.clone(),
            staging: staging.clone(),
        };
        assert!(
            launcher
                .launch(LaunchRequest {
                    program: "codex".into(),
                    args: vec!["app-server".into()],
                    env: BTreeMap::from([
                        ("ORCH_LLM_TOKEN".into(), "synthetic-orch-key".into()),
                        (
                            "UNRELATED_PRIVATE_VALUE".into(),
                            "another-person-value".into()
                        ),
                        ("HOME".into(), "/person/home".into()),
                    ]),
                    cwd: Some("/workspace/private-project".into())
                })
                .await
                .is_err()
        );
        let files = capture.files.lock().unwrap();
        assert_eq!(
            files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>(),
            ["home/auth.json", "home/config.toml"]
        );
        assert_eq!(files[0].bytes, b"synthetic-selected-signin");
        assert!(
            String::from_utf8(files[1].bytes.clone())
                .unwrap()
                .contains("/work/home/history")
        );
        let process = capture.process.lock().unwrap();
        let process = process.as_ref().unwrap();
        assert_eq!(
            process.env,
            BTreeMap::from([("CODEX_HOME".into(), "/work/home".into())])
        );
        assert_eq!(process.cwd, None);
    }
}
