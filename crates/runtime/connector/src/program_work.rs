//! Private work IO stays with the trusted connector; the native program receives no channel credential.
use async_trait::async_trait;
use orch_agent_contract::{adapter::AdapterError, program_work::ProgramIo};
use orch_driver_contract::{
    driver::DriverError,
    work::{WorkFile, WorkOutput, WorkProcess, WorkProcessSpec},
};
use orch_link::program_work::{Answer, Client};
use orch_link_contract::program_work::{File, Reply, Request};
use orch_types::{Hash, Id};
use std::sync::Arc;

pub(crate) struct Work {
    client: Client,
    work: Id,
    input_files: Vec<(Hash, std::path::PathBuf)>,
}
impl Work {
    pub(crate) fn new(
        client: Client,
        work: Id,
        input_files: Vec<(Hash, std::path::PathBuf)>,
    ) -> Arc<Self> {
        Arc::new(Self {
            client,
            work,
            input_files,
        })
    }
}
fn adapter_error(error: impl std::fmt::Display) -> AdapterError {
    AdapterError::HarnessUnavailable(error.to_string())
}
fn answer(answer: Answer) -> Result<Answer, DriverError> {
    match answer.reply {
        Reply::Failed { ref detail, .. } => Err(DriverError::Unavailable(detail.clone())),
        _ => Ok(answer),
    }
}
fn wrong() -> DriverError {
    DriverError::Unavailable("the isolated program channel returned a different operation".into())
}

#[async_trait]
impl ProgramIo for Work {
    async fn prepare(&self, mut files: Vec<WorkFile>) -> Result<(), AdapterError> {
        for (blob, cached) in &self.input_files {
            let bytes = tokio::fs::read(cached).await.map_err(adapter_error)?;
            if Hash::of(&bytes) != *blob {
                return Err(adapter_error(
                    "The frozen unit input no longer matches its content hash.",
                ));
            }
            files.push(WorkFile {
                path: orch_link_contract::program_work::input_path(*blob),
                bytes,
            });
        }
        let header = files
            .iter()
            .map(|file| {
                Ok(File {
                    path: file.path.clone(),
                    bytes: u64::try_from(file.bytes.len()).map_err(adapter_error)?,
                })
            })
            .collect::<Result<Vec<_>, AdapterError>>()?;
        let body: Vec<&[u8]> = files.iter().map(|file| file.bytes.as_slice()).collect();
        let response = answer(
            self.client
                .request(&Request::Prepare { files: header }, &body)
                .await
                .map_err(adapter_error)?,
        )
        .map_err(adapter_error)?;
        if response.reply != Reply::Ready {
            return Err(adapter_error(wrong()));
        }
        Ok(())
    }
    async fn launch(&self, process: WorkProcessSpec) -> Result<Arc<dyn WorkProcess>, AdapterError> {
        let frozen = serde_json::to_vec(&(self.work, &process)).map_err(adapter_error)?;
        let hash = Hash::of(&frozen);
        // UUID's 128-bit representation supplies the invocation identity, not permission or a workload cap.
        let mut identity = [0_u8; 16];
        identity.copy_from_slice(&hash.as_bytes()[..16]);
        let invocation = Id::from_bytes(identity);
        let response = answer(
            self.client
                .request(
                    &Request::Start {
                        invocation,
                        process: process.clone(),
                    },
                    &[],
                )
                .await
                .map_err(adapter_error)?,
        )
        .map_err(adapter_error)?;
        let Reply::Started { reference } = response.reply else {
            return Err(adapter_error(wrong()));
        };
        Ok(Arc::new(Process {
            client: self.client.clone(),
            invocation,
            spec: process,
            reference,
            input: tokio::sync::Mutex::new(0),
            output: tokio::sync::Mutex::new(0),
        }))
    }
    async fn read_file(&self, path: &str, max_bytes: u64) -> Result<Vec<u8>, AdapterError> {
        let response = answer(
            self.client
                .request(
                    &Request::ReadFile {
                        path: path.to_owned(),
                        max_bytes,
                    },
                    &[],
                )
                .await
                .map_err(adapter_error)?,
        )
        .map_err(adapter_error)?;
        if !matches!(response.reply, Reply::File { .. }) {
            return Err(adapter_error(wrong()));
        }
        Ok(response.bytes)
    }
    async fn retire(&self) -> Result<(), AdapterError> {
        let response = answer(
            self.client
                .request(&Request::Retire, &[])
                .await
                .map_err(adapter_error)?,
        )
        .map_err(adapter_error)?;
        if response.reply != Reply::Ready {
            return Err(adapter_error(wrong()));
        }
        Ok(())
    }
}

struct Process {
    client: Client,
    invocation: Id,
    spec: WorkProcessSpec,
    reference: String,
    input: tokio::sync::Mutex<u64>,
    output: tokio::sync::Mutex<u64>,
}
impl Process {
    async fn call(&self, request: &Request, body: &[&[u8]]) -> Result<Answer, DriverError> {
        answer(
            self.client
                .request(request, body)
                .await
                .map_err(|error| DriverError::Unavailable(error.to_string()))?,
        )
    }
}
#[async_trait]
impl WorkProcess for Process {
    fn reference(&self) -> &str {
        &self.reference
    }
    async fn start(&self) -> Result<(), DriverError> {
        let response = self
            .call(
                &Request::Start {
                    invocation: self.invocation,
                    process: self.spec.clone(),
                },
                &[],
            )
            .await?;
        match response.reply {
            Reply::Started { reference } if reference == self.reference => Ok(()),
            _ => Err(wrong()),
        }
    }
    async fn write(&self, bytes: &[u8]) -> Result<(), DriverError> {
        let mut offset = self.input.lock().await;
        for bytes in bytes.chunks(self.client.chunk_bytes()) {
            let length = u64::try_from(bytes.len()).map_err(|_| wrong())?;
            let response = self
                .call(
                    &Request::Write {
                        invocation: self.invocation,
                        offset: *offset,
                        bytes: length,
                    },
                    &[bytes],
                )
                .await?;
            let next = offset.checked_add(length).ok_or_else(wrong)?;
            if response.reply != (Reply::Written { next }) {
                return Err(wrong());
            }
            *offset = next;
        }
        Ok(())
    }
    async fn close_input(&self) -> Result<(), DriverError> {
        let response = self
            .call(
                &Request::CloseInput {
                    invocation: self.invocation,
                },
                &[],
            )
            .await?;
        if response.reply != Reply::Ready {
            return Err(wrong());
        }
        Ok(())
    }
    async fn next_output(&self) -> Result<Option<WorkOutput>, DriverError> {
        let mut offset = self.output.lock().await;
        let response = self
            .call(
                &Request::Output {
                    invocation: self.invocation,
                    offset: *offset,
                },
                &[],
            )
            .await?;
        match response.reply {
            Reply::End { offset: at } if at == *offset => Ok(None),
            Reply::Output {
                offset: at,
                stream,
                bytes,
            } if at == *offset && usize::try_from(bytes).ok() == Some(response.bytes.len()) => {
                *offset = offset.checked_add(bytes).ok_or_else(wrong)?;
                Ok(Some(WorkOutput {
                    stream,
                    bytes: response.bytes,
                }))
            }
            _ => Err(wrong()),
        }
    }
    async fn wait(&self) -> Result<Option<i64>, DriverError> {
        let response = self
            .call(
                &Request::Wait {
                    invocation: self.invocation,
                },
                &[],
            )
            .await?;
        match response.reply {
            Reply::Exited { code } => Ok(code),
            _ => Err(wrong()),
        }
    }
    async fn terminate(&self) -> Result<(), DriverError> {
        let response = self.call(&Request::Retire, &[]).await?;
        if response.reply != Reply::Ready {
            return Err(wrong());
        }
        Ok(())
    }
}
