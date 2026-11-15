//! Trusted connector control of an isolated program over the existing signed, replayable transport.
use crate::{machine::MachineLink, tunnels::channel::OpenError};
use orch_link_contract::program_work::{Reply, Request};
use serde::{Serialize, de::DeserializeOwned};
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The native bytes are kept apart from errors and metadata.
pub struct Answer {
    pub reply: Reply,
    pub bytes: Vec<u8>,
}
impl std::fmt::Debug for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramAnswer")
            .field("reply", &self.reply)
            .field("bytes", &"[private]")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the isolated program channel could not be opened: {0}")]
    Open(#[from] OpenError),
    #[error("the isolated program channel ended: {0}")]
    Io(#[from] io::Error),
}

/// Only the trusted adapter owns this ticket. It never enters the bare program or its home.
#[derive(Clone)]
pub struct Client {
    link: MachineLink,
    ticket: String,
}
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProgramClient").finish_non_exhaustive()
    }
}
impl Client {
    pub fn new(link: MachineLink, ticket: String) -> Self {
        Self { link, ticket }
    }

    /// A retried control operation keeps its invocation/byte offset; it never infers a new model turn.
    pub async fn request(&self, request: &Request, body: &[&[u8]]) -> Result<Answer, Error> {
        let expected = request
            .input_bytes()
            .ok_or_else(|| io::Error::other("program input size overflow"))?;
        let actual = body
            .iter()
            .try_fold(0_u64, |total, part| {
                total.checked_add(u64::try_from(part.len()).ok()?)
            })
            .ok_or_else(|| io::Error::other("program input size overflow"))?;
        if expected != actual {
            return Err(
                io::Error::other("the program body differs from its frozen file framing").into(),
            );
        }
        let mut tunnel = self.link.open_tunnel(&self.ticket).await?;
        let limit = self.link.message_bytes();
        write_json(&mut tunnel, request, limit).await?;
        for part in body {
            tunnel.write_all(part).await?;
        }
        tunnel.flush().await?;
        let reply: Reply = read_json(&mut tunnel, limit).await?;
        let length = reply.output_bytes();
        let budget = match request {
            Request::ReadFile { max_bytes, .. } => *max_bytes,
            _ => u64::try_from(limit).unwrap_or(u64::MAX),
        };
        if length > budget {
            return Err(
                io::Error::other("the program answer exceeds its requested byte allowance").into(),
            );
        }
        let mut bytes = vec![
            0;
            usize::try_from(length).map_err(|_| io::Error::other(
                "the program answer cannot fit this platform"
            ))?
        ];
        tunnel.read_exact(&mut bytes).await?;
        Ok(Answer { reply, bytes })
    }
}

/// Headers use the link's existing configurable message allowance; bulk file bytes are streamed separately.
pub async fn read_json<T: DeserializeOwned>(
    read: &mut (impl AsyncRead + Unpin),
    limit: usize,
) -> io::Result<T> {
    let length = usize::try_from(read.read_u32().await?)
        .map_err(|_| io::Error::other("program header length cannot fit this platform"))?;
    if length > limit {
        return Err(io::Error::other(
            "the program header exceeds the current link allowance",
        ));
    }
    let mut bytes = vec![0; length];
    read.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::other("the complete program header is invalid"))
}

pub async fn write_json(
    write: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
    limit: usize,
) -> io::Result<()> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| io::Error::other("the program header is invalid"))?;
    let length = u32::try_from(bytes.len())
        .map_err(|_| io::Error::other("the program header exceeds its wire representation"))?;
    if bytes.len() > limit {
        return Err(io::Error::other(
            "the program header exceeds the current link allowance",
        ));
    }
    write.write_u32(length).await?;
    write.write_all(&bytes).await?;
    write.flush().await
}
