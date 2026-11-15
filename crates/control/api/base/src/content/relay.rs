//! The command-bound memory endpoint, beside the stored-content endpoint on the internal listener.

use super::*;
use crate::relay::{Relay, RelayError};
use std::sync::Arc;
use std::time::Duration;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Position {
    #[serde(default)]
    offset: i64,
    len: Option<i64>,
    received: Option<i64>,
}

pub(super) fn routes() -> Router<App> {
    Router::new().route(
        "/internal/link/blobs/relay/{relay}",
        get(pull).head(stand).put(push),
    )
}

async fn authorized(
    app: &App,
    peer: &Peer,
    headers: &HeaderMap,
    id: Id,
    wanted: CommandKind,
) -> Result<(Arc<Relay>, Duration), LinkFault> {
    let asker = asker(app, peer, headers).await?;
    let Named::Open { kind, body } = &asker.command else {
        return Err(LinkFault::forbidden(
            "transit bytes require their own open command",
        ));
    };
    let named = body
        .get("relay_id")
        .and_then(Json::as_str)
        .and_then(|value| value.parse::<Id>().ok());
    if *kind != wanted || named != Some(id) {
        return Err(LinkFault::forbidden(
            "that command does not own this direction of this transfer",
        ));
    }
    let idle = body
        .get("idle_secs")
        .and_then(Json::as_u64)
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| LinkFault::invalid("the transfer names no progress allowance"))?;
    let relay = app
        .blobs
        .relays()
        .get(id)
        .ok_or_else(|| LinkFault::new(410, "gone", "the transfer's meeting place is gone"))?;
    Ok((relay, Duration::from_secs(idle)))
}

fn identity(value: &str) -> Result<Id, LinkFault> {
    value
        .parse()
        .map_err(|_| LinkFault::invalid("the transfer is not an identifier"))
}

fn nonnegative(value: i64) -> Result<u64, LinkFault> {
    u64::try_from(value).map_err(|_| LinkFault::invalid("a file offset cannot be negative"))
}

fn failed(error: RelayError) -> LinkFault {
    match error {
        RelayError::Ended => LinkFault::new(410, "gone", "the other end of this transfer is gone"),
        RelayError::Range => {
            LinkFault::invalid("the requested range does not belong to this transfer")
        }
        RelayError::Offset { received } => {
            LinkFault::invalid(format!("the destination already holds {received} bytes"))
        }
    }
}

/// A destination reports only what it has written, including the last piece with no following GET.
async fn stand(
    State(app): State<App>,
    Extension(peer): Extension<Peer>,
    Path(id): Path<String>,
    Query(position): Query<Position>,
    headers: HeaderMap,
) -> Result<Response, LinkFault> {
    let (relay, _) = authorized(
        &app,
        &peer,
        &headers,
        identity(&id)?,
        CommandKind::FsRelayReceive,
    )
    .await?;
    let received = position
        .received
        .ok_or_else(|| LinkFault::invalid("the destination reports no prefix"))?;
    relay.stand(nonnegative(received)?).map_err(failed)?;
    Ok(progress_reply(
        StatusCode::OK,
        BlobProgress {
            received,
            complete: false,
        },
    ))
}

async fn pull(
    State(app): State<App>,
    Extension(peer): Extension<Peer>,
    Path(id): Path<String>,
    Query(position): Query<Position>,
    headers: HeaderMap,
) -> Result<Response, LinkFault> {
    let id = identity(&id)?;
    let (relay, idle) = authorized(&app, &peer, &headers, id, CommandKind::FsRelayReceive).await?;
    let offset = nonnegative(position.offset)?;
    let len = nonnegative(
        position
            .len
            .ok_or_else(|| LinkFault::invalid("a transfer read names its length"))?,
    )?;
    if len == 0 || len > u64::try_from(MAX_CHUNK_BYTES).unwrap_or(u64::MAX) {
        return Err(LinkFault::invalid(
            "the read is not one bounded content piece",
        ));
    }
    // Return an empty poll before the peer's request allowance expires. This fraction is a
    // transport heartbeat, not a transfer lifetime or a limit on a person's work.
    let found = tokio::time::timeout(idle / 2, relay.pull(offset, len)).await;
    // A command may have been cancelled while this empty poll waited.
    authorized(&app, &peer, &headers, id, CommandKind::FsRelayReceive).await?;
    match found {
        Ok(Ok(Some(piece))) => Ok((StatusCode::OK, piece).into_response()),
        Ok(Ok(None)) => Ok(StatusCode::RESET_CONTENT.into_response()),
        Ok(Err(error)) => Err(failed(error)),
        Err(_) => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}

async fn push(
    State(app): State<App>,
    Extension(peer): Extension<Peer>,
    Path(id): Path<String>,
    Query(position): Query<Position>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, LinkFault> {
    let id = identity(&id)?;
    let (relay, idle) = authorized(&app, &peer, &headers, id, CommandKind::FsRelaySend).await?;
    // Acquire the producer slot before collecting its bounded body. Concurrent retries of one
    // command never allocate extra whole pieces while the browser is slow.
    let input = relay.input().await;
    authorized(&app, &peer, &headers, id, CommandKind::FsRelaySend).await?;
    let bytes = to_bytes(
        body,
        usize::try_from(MAX_CHUNK_BYTES).unwrap_or(usize::MAX) + 1,
    )
    .await
    .map_err(|_| {
        LinkFault::new(
            413,
            "request.too_large",
            "a transfer request carries one bounded content piece",
        )
    })?;
    if i64::try_from(bytes.len()).unwrap_or(i64::MAX) > MAX_CHUNK_BYTES {
        return Err(LinkFault::new(
            413,
            "request.too_large",
            "a transfer request carries one bounded content piece",
        ));
    }
    authorized(&app, &peer, &headers, id, CommandKind::FsRelaySend).await?;
    let received =
        match tokio::time::timeout(idle, input.push(nonnegative(position.offset)?, bytes)).await {
            Ok(Ok(received)) => received,
            Ok(Err(RelayError::Offset { received })) => {
                return Ok(progress_reply(
                    StatusCode::CONFLICT,
                    BlobProgress {
                        received: i64::try_from(received).unwrap_or(i64::MAX),
                        complete: false,
                    },
                ));
            }
            Ok(Err(error)) => return Err(failed(error)),
            Err(_) => {
                return Err(LinkFault::new(
                    408,
                    "timeout",
                    "the relay reader did not acknowledge a piece within its allowance",
                ));
            }
        };
    Ok(progress_reply(
        StatusCode::OK,
        BlobProgress {
            received: i64::try_from(received).unwrap_or(i64::MAX),
            complete: false,
        },
    ))
}
