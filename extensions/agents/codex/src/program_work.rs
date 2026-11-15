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
    pub turn: Option<orch_types::Id>,
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
            adapter::AdapterError,
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
            return self.open(spec, None, None).await;
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
        let mut running = isolated
            .launch_model(start, events, None, Some(identity))
            .await?;
        running.home = Some(staging.path().to_path_buf());
        running.shared.set_program_io(io.clone());
        self.model_processes.insert(identity, running);
        self.unit_work.insert(
            thread,
            Context {
                identity,
                io: io.clone(),
                turn: None,
                _staging: staging,
            },
        );
        match self.open(spec, None, None).await {
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

pub(super) fn input_image(
    image: &orch_agent_contract::session::ImageInput,
) -> Result<orch_types::Json, orch_agent_contract::adapter::AdapterError> {
    use base64::Engine as _;
    use orch_agent_contract::adapter::AdapterError;
    let path = image
        .path
        .as_ref()
        .ok_or_else(|| AdapterError::NotFound("the frozen image file is unavailable".into()))?;
    let bytes =
        std::fs::read(path).map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;
    if orch_types::Hash::of(&bytes) != image.blob || !image.media_type.starts_with("image/") {
        return Err(AdapterError::HarnessError(
            "the frozen image bytes differ from their accepted identity".into(),
        ));
    }
    Ok(
        serde_json::json!({"type":"image","url":format!("data:{};base64,{}",image.media_type,base64::engine::general_purpose::STANDARD.encode(bytes))}),
    )
}
struct Control(Arc<dyn WorkProcess>);

/// A native path from the isolated process never becomes a read on the connector's filesystem.
pub(super) async fn prepare_image(io: &dyn ProgramIo, method: &str, params: &mut orch_types::Json) {
    use base64::Engine as _;
    use orch_types::Json;
    if !matches!(method, "item/started" | "item/completed") {
        return;
    }
    let Some(item) = params.get_mut("item").and_then(Json::as_object_mut) else {
        return;
    };
    let generated = item.get("type").and_then(Json::as_str) == Some("imageGeneration");
    let viewed = item.get("type").and_then(Json::as_str) == Some("imageView");
    if !generated && !viewed {
        return;
    }
    let key = if generated { "savedPath" } else { "path" };
    let Some(path) = item
        .remove(key)
        .and_then(|value| value.as_str().map(str::to_owned))
        .filter(|path| !path.is_empty())
    else {
        return;
    };
    // Only the generated-image result is published by this capability. Native view paths grant no local read.
    if !generated || method != "item/completed" {
        return;
    }
    item.remove("result");
    let path = std::path::Path::new(&path);
    let Ok(relative) = path.strip_prefix("/work") else {
        return;
    };
    let Some(relative) = relative.to_str() else {
        return;
    };
    let media = match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("gif") => "image/gif",
        _ => return,
    };
    // Stored blob size is signed 64-bit; this is its representation bound, not a model/workload policy.
    let Ok(bytes) = io.read_file(relative, i64::MAX as u64).await else {
        return;
    };
    item.insert(
        "result".into(),
        Json::String(format!(
            "data:{media};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )),
    );
}

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
        reads: Mutex<Vec<String>>,
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
        async fn read_file(&self, path: &str, _: u64) -> Result<Vec<u8>, AdapterError> {
            self.reads.lock().unwrap().push(path.to_owned());
            Ok(b"frozen unit result".to_vec())
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

    #[tokio::test]
    async fn only_the_isolated_generated_file_is_read_and_a_native_path_cannot_escape_to_the_connector()
     {
        let io = Capture::default();
        let mut frame = serde_json::json!({"item":{"type":"imageGeneration","status":"completed","savedPath":"/work/result.png"}});
        prepare_image(&io, "item/completed", &mut frame).await;
        assert_eq!(*io.reads.lock().unwrap(), ["result.png"]);
        assert!(frame["item"].get("savedPath").is_none());
        assert!(
            frame["item"]["result"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        let mut escaped = serde_json::json!({"item":{"type":"imageGeneration","savedPath":"/workspace/private.png"}});
        prepare_image(&io, "item/completed", &mut escaped).await;
        assert!(escaped["item"].get("savedPath").is_none());
        assert!(escaped["item"].get("result").is_none());
        assert_eq!(io.reads.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_frozen_image_crosses_as_native_image_data_without_a_personal_filesystem_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("input.png");
        std::fs::write(&path, b"accepted image bytes").unwrap();
        let mut image = orch_agent_contract::session::ImageInput {
            blob: orch_types::Hash::of(b"accepted image bytes"),
            media_type: "image/png".into(),
            name: "input.png".into(),
            workspace_path: Some("/workspace/private.png".into()),
            path: Some(path.to_string_lossy().into_owned()),
        };
        let input = input_image(&image).unwrap();
        assert_eq!(input["type"], "image");
        assert!(
            input["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/png;base64,")
        );
        assert!(!input.to_string().contains("/workspace"));
        image.blob = orch_types::Hash::of(b"changed bytes");
        assert!(input_image(&image).is_err());
    }
}
