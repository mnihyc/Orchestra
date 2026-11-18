//! The trusted adapter keeps process control and reads only bounded scratch results.
use crate::{
    DockerDriver,
    client::map_error,
    restricted,
    work_units::{LABEL_DEADLINE, LABEL_INPUT, LABEL_WORK, PREPARED, SCRATCH, WORK_UID},
};
use async_trait::async_trait;
use bollard::{container::LogOutput, exec::StartExecResults, models::ExecConfig};
use futures::{Stream, StreamExt, TryStreamExt};
use orch_driver_contract::{
    driver::DriverError,
    units::UnitRef,
    work::{WorkOutput, WorkOutputStream, WorkProcess, WorkProcessSpec},
};
use orch_types::Time;
use std::{pin::Pin, sync::Arc, time::Duration};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

type Output = Pin<Box<dyn Stream<Item = Result<LogOutput, bollard::errors::Error>> + Send>>;
struct Process {
    driver: DockerDriver,
    unit: UnitRef,
    reference: String,
    state: Mutex<State>,
}
struct Streams {
    input: Mutex<Pin<Box<dyn AsyncWrite + Send>>>,
    output: Mutex<Output>,
}
enum State {
    Prepared,
    Starting,
    Attached(Arc<Streams>),
}
impl Process {
    async fn streams(&self) -> Result<Arc<Streams>, DriverError> {
        match &*self.state.lock().await {
            State::Attached(streams) => Ok(streams.clone()),
            State::Prepared | State::Starting => Err(DriverError::Unavailable("the isolated program's streams are not attached; an uncertain start was not repeated".into())),
        }
    }
}

#[async_trait]
impl WorkProcess for Process {
    fn reference(&self) -> &str {
        &self.reference
    }
    async fn start(&self) -> Result<(), DriverError> {
        let mut state = self.state.lock().await;
        match &*state {
            State::Attached(_) => return Ok(()),
            State::Starting => {
                return Err(DriverError::Unavailable(
                    "the first isolated start has an uncertain outcome; no new invocation was made"
                        .into(),
                ));
            }
            State::Prepared => {}
        }
        self.driver.current_work(&self.unit).await?;
        *state = State::Starting;
        let StartExecResults::Attached { input, output } = self
            .driver
            .daemon
            .docker()
            .start_exec(&self.reference, None)
            .await
            .map_err(map_error)?
        else {
            return Err(DriverError::Unavailable(
                "the isolated program did not attach its standard streams".into(),
            ));
        };
        *state = State::Attached(Arc::new(Streams {
            input: Mutex::new(input),
            output: Mutex::new(output),
        }));
        Ok(())
    }
    async fn write(&self, bytes: &[u8]) -> Result<(), DriverError> {
        let streams = self.streams().await?;
        let mut input = streams.input.lock().await;
        input
            .write_all(bytes)
            .await
            .map_err(|_| DriverError::Unavailable("the isolated program's input closed".into()))?;
        input
            .flush()
            .await
            .map_err(|_| DriverError::Unavailable("the isolated program's input closed".into()))
    }
    async fn close_input(&self) -> Result<(), DriverError> {
        self.streams()
            .await?
            .input
            .lock()
            .await
            .shutdown()
            .await
            .map_err(|_| {
                DriverError::Unavailable("the isolated program's input could not be closed".into())
            })
    }
    async fn next_output(&self) -> Result<Option<WorkOutput>, DriverError> {
        let streams = self.streams().await?;
        let Some(output) = streams.output.lock().await.next().await else {
            return Ok(None);
        };
        let output = output.map_err(map_error)?;
        let stream = match output {
            LogOutput::StdErr { .. } => WorkOutputStream::Stderr,
            LogOutput::StdOut { .. } | LogOutput::StdIn { .. } | LogOutput::Console { .. } => {
                WorkOutputStream::Stdout
            }
        };
        Ok(Some(WorkOutput {
            stream,
            bytes: output.into_bytes().to_vec(),
        }))
    }
    async fn wait(&self) -> Result<Option<i64>, DriverError> {
        loop {
            let state = self
                .driver
                .daemon
                .docker()
                .inspect_exec(&self.reference)
                .await
                .map_err(map_error)?;
            if state.running != Some(true) {
                return Ok(state.exit_code);
            }
            tokio::time::sleep(crate::egress::program::POLL).await;
        }
    }
    async fn terminate(&self) -> Result<(), DriverError> {
        self.driver.stop_unit(&self.unit, Duration::ZERO).await
    }
}

fn relative(path: &str) -> bool {
    !path.contains(['\\', '\0'])
        && path
            .split('/')
            .all(|part| !part.is_empty() && !matches!(part, "." | ".."))
}

impl DockerDriver {
    async fn current_work(&self, unit: &UnitRef) -> Result<(), DriverError> {
        let container = self
            .container_of(unit)
            .await?
            .ok_or_else(|| DriverError::NotFound("the isolated work unit is absent".into()))?;
        let labels = container
            .config
            .as_ref()
            .and_then(|config| config.labels.as_ref())
            .ok_or_else(|| DriverError::InvalidSpec("this is not an isolated work unit".into()))?;
        if labels.get(LABEL_WORK) != Some(&unit.runtime_id.to_string())
            || container.state.as_ref().and_then(|state| state.running) != Some(true)
        {
            return Err(DriverError::Unavailable(
                "the isolated work unit is not running".into(),
            ));
        }
        let deadline: Time = labels
            .get(LABEL_DEADLINE)
            .ok_or_else(|| DriverError::InvalidSpec("the work has no lifetime".into()))?
            .parse()
            .map_err(|_| DriverError::InvalidSpec("the work's lifetime is unreadable".into()))?;
        if self.clock.time() >= deadline {
            return Err(DriverError::Timeout);
        }
        let prepared: Option<String> = crate::egress::files::read_json(
            &restricted::unit_dir(&self.config, &unit.name)?,
            PREPARED,
        )
        .await;
        if prepared.is_none() || prepared.as_ref() != labels.get(LABEL_INPUT) {
            return Err(DriverError::Unavailable(
                "the isolated program's frozen input is not prepared".into(),
            ));
        }
        Ok(())
    }

    pub(crate) async fn prepare_work_invocation(
        &self,
        unit: &UnitRef,
        process: &WorkProcessSpec,
    ) -> Result<Arc<dyn WorkProcess>, DriverError> {
        self.current_work(unit).await?;
        if process.program.is_empty()
            || process.program.contains('\0')
            || process.args.iter().any(|argument| argument.contains('\0'))
            || process.cwd.as_deref().is_some_and(|path| !relative(path))
            || process.env.values().any(|value| value.contains('\0'))
            || process.env.keys().any(|key| {
                key.is_empty()
                    || key.contains(['=', '\0'])
                    || key.to_ascii_uppercase().starts_with("ORCH_")
            })
        {
            return Err(DriverError::InvalidSpec("an isolated invocation needs a program, a scratch working directory and no Orch credential variables".into()));
        }
        let mut command = vec![process.program.clone()];
        command.extend(process.args.clone());
        // Proxy and home variables are fixed by the unit, independent of adapter configuration.
        let mut env: Vec<_> = process
            .env
            .iter()
            .filter(|(key, _)| {
                !matches!(
                    key.to_ascii_lowercase().as_str(),
                    "http_proxy"
                        | "https_proxy"
                        | "all_proxy"
                        | "no_proxy"
                        | "home"
                        | "tmpdir"
                        | "path"
                )
            })
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        env.sort();
        let created =
            self.daemon
                .docker()
                .create_exec(
                    &unit.reference,
                    ExecConfig {
                        attach_stdin: Some(true),
                        attach_stdout: Some(true),
                        attach_stderr: Some(true),
                        tty: Some(false),
                        privileged: Some(false),
                        user: Some(format!("{WORK_UID}:{WORK_UID}")),
                        working_dir: Some(process.cwd.as_ref().map_or_else(
                            || SCRATCH.to_owned(),
                            |path| format!("{SCRATCH}/{path}"),
                        )),
                        cmd: Some(command),
                        env: Some(env),
                        ..ExecConfig::default()
                    },
                )
                .await
                .map_err(map_error)?;
        Ok(Arc::new(Process {
            driver: self.clone(),
            unit: unit.clone(),
            reference: created.id,
            state: Mutex::new(State::Prepared),
        }))
    }

    pub(crate) async fn read_work_result(
        &self,
        unit: &UnitRef,
        path: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, DriverError> {
        self.current_work(unit).await?;
        if !relative(path) {
            return Err(DriverError::InvalidSpec(
                "a result needs a relative scratch file".into(),
            ));
        }
        let name = std::path::Path::new(path)
            .file_name()
            .ok_or_else(|| DriverError::InvalidSpec("the result has no filename".into()))?
            .to_owned();
        let options = bollard::query_parameters::DownloadFromContainerOptionsBuilder::default()
            .path(&format!("{SCRATCH}/{path}"))
            .build();
        let docker = self.daemon.docker().clone();
        let reference = unit.reference.clone();
        tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let stream = Box::pin(docker.download_from_container(&reference, Some(options)).map_err(std::io::Error::other));
            let reader = tokio_util::io::SyncIoBridge::new(tokio_util::io::StreamReader::new(stream));
            let mut archive = tar::Archive::new(reader);
            let mut result = None;
            for entry in archive.entries().map_err(|_|DriverError::Unavailable("the result archive is unreadable".into()))? {
                let entry = entry.map_err(|_|DriverError::Unavailable("the result archive is unreadable".into()))?;
                if !entry.header().entry_type().is_file()
                    || entry.path().ok().and_then(|path|path.file_name().map(std::ffi::OsStr::to_owned)).as_ref() != Some(&name)
                    || entry.size() > max_bytes || result.is_some()
                {
                    return Err(DriverError::InvalidSpec("the result is not one regular scratch file within its accepted byte budget".into()));
                }
                let mut bytes = Vec::new();
                entry.take(max_bytes.saturating_add(1)).read_to_end(&mut bytes)
                    .map_err(|_|DriverError::Unavailable("the result file could not be read".into()))?;
                if u64::try_from(bytes.len()).map_err(|_|DriverError::InvalidSpec("the result length cannot be represented".into()))? > max_bytes {
                    return Err(DriverError::InvalidSpec("the result exceeds its accepted byte budget".into()));
                }
                result = Some(bytes);
            }
            result.ok_or_else(||DriverError::NotFound("the regular scratch result is absent".into()))
        }).await.map_err(|_|DriverError::Unavailable("the result reader ended".into()))?
    }
}
