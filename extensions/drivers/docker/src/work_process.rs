//! The trusted adapter keeps process control and reads only bounded scratch results.
use crate::{
    DockerDriver,
    client::map_error,
    restricted,
    work_units::{LABEL_DEADLINE, LABEL_INPUT, LABEL_WORK, PREPARED, SCRATCH, WORK_UID},
};
use async_trait::async_trait;
use bollard::{container::LogOutput, exec::StartExecResults, models::ExecConfig};
use futures::{Stream, StreamExt};
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
        self.driver.wait_work_exec(&self.reference).await
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
    pub(crate) async fn wait_work_exec(&self, reference: &str) -> Result<Option<i64>, DriverError> {
        loop {
            let state = self
                .daemon
                .docker()
                .inspect_exec(reference)
                .await
                .map_err(map_error)?;
            if state.running != Some(true) {
                return Ok(state.exit_code);
            }
            tokio::time::sleep(crate::egress::program::POLL).await;
        }
    }

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
        if !relative(path) {
            return Err(DriverError::InvalidSpec(
                "a result needs a relative scratch file".into(),
            ));
        }
        // Inspect the opened descriptor in the unit. A path checked before opening can change
        // while a native program is finishing its output. Only that same descriptor is copied.
        const READ: &str = r#"set -eu
file="./$1"
[ -f "$file" ] && [ ! -L "$file" ] || exit 1
exec 3<"$file"
[ -f /proc/self/fd/3 ] || exit 1
case "$(readlink /proc/self/fd/3)" in /work/*) ;; *) exit 1 ;; esac
exec head -c "$2" <&3"#;
        let process = self
            .prepare_work_invocation(
                unit,
                &WorkProcessSpec {
                    program: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        READ.into(),
                        "read-result".into(),
                        path.into(),
                        max_bytes.saturating_add(1).min(i64::MAX as u64).to_string(),
                    ],
                    env: Default::default(),
                    cwd: None,
                },
            )
            .await?;
        process.start().await?;
        let mut bytes = Vec::new();
        while let Some(chunk) = process.next_output().await? {
            if chunk.stream != WorkOutputStream::Stdout {
                continue;
            }
            let length = bytes
                .len()
                .checked_add(chunk.bytes.len())
                .and_then(|n| u64::try_from(n).ok());
            if length.is_none_or(|n| n > max_bytes) {
                return Err(DriverError::InvalidSpec(
                    "the result exceeds its accepted byte budget".into(),
                ));
            }
            bytes.try_reserve(chunk.bytes.len()).map_err(|_| {
                DriverError::NoCapacity("the scratch result does not fit in memory".into())
            })?;
            bytes.extend_from_slice(&chunk.bytes);
        }
        if process.wait().await? != Some(0) {
            return Err(DriverError::InvalidSpec(
                "the result is not a readable regular scratch file".into(),
            ));
        }
        Ok(bytes)
    }
}
