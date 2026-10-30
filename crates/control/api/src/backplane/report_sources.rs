//! Session sources checked before stream ordering, effects and cursor writes.

use orch_db::{Conn, DbError};
use orch_link_contract::backplane::UpFrame;
use orch_link_contract::streams::Stream;
use orch_types::Id;

/// Whether this machine is the recorded source of a session stream.
pub(super) async fn owns(conn: &mut Conn, machine: Id, stream: &Stream) -> Result<bool, DbError> {
    orch_db::queries::machine_reports::owns(conn, machine, stream).await
}

/// Checks a frame before stream ordering or transient publication.
pub(super) async fn accepts_frame(
    conn: &mut Conn,
    machine: Id,
    stream: &Stream,
    frame: &UpFrame,
) -> Result<bool, DbError> {
    use orch_db::queries::machine_reports::accepts;
    match frame {
        UpFrame::Ev(event) => {
            accepts(conn, machine, stream, &[(event.kind.as_str(), &event.body)]).await
        }
        UpFrame::Evb(batch) => {
            accepts(
                conn,
                machine,
                stream,
                &batch
                    .events
                    .iter()
                    .map(|event| (event.kind.as_str(), &event.body))
                    .collect::<Vec<_>>(),
            )
            .await
        }
        UpFrame::Eph(report) => {
            accepts(
                conn,
                machine,
                stream,
                &[(report.kind.as_str(), &report.body)],
            )
            .await
        }
        _ => owns(conn, machine, stream).await,
    }
}

/// A frame's stream and the last durable number it needs acknowledged.
pub(super) fn frame_stream(frame: &UpFrame) -> Option<(&str, Option<i64>)> {
    match frame {
        UpFrame::Ev(event) => Some((&event.stream, Some(event.seq))),
        UpFrame::Evb(batch) => Some((
            &batch.stream,
            batch.events.iter().map(|event| event.seq).max(),
        )),
        UpFrame::Reset(reset) => Some((&reset.stream, None)),
        UpFrame::Eph(report) => Some((&report.stream, None)),
        _ => None,
    }
}

/// The first and last numbers of a durable frame, before it enters ordering.
pub(super) fn frame_range(frame: &UpFrame) -> Option<(i64, i64)> {
    match frame {
        UpFrame::Ev(event) => Some((event.seq, event.seq)),
        UpFrame::Evb(batch) => Some((
            batch.events.first()?.seq,
            batch.events.iter().map(|event| event.seq).max()?,
        )),
        _ => None,
    }
}

/// How many reports a rejected frame discards, counting every event in a batch.
pub(super) fn frame_count(frame: &UpFrame) -> usize {
    match frame {
        UpFrame::Evb(batch) => batch.events.len(),
        UpFrame::Ev(_) | UpFrame::Eph(_) | UpFrame::Reset(_) => 1,
        _ => 0,
    }
}
