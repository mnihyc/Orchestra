//! The headless test screen answers the initial cursor query before it types a command.

use std::time::Duration;

use orch_envhost::terminal::{Attachment, Event};

pub async fn answer_initial_query(attachment: &mut Attachment) {
    const QUERY: &[u8] = b"\x1b[6n";
    let mut pending = attachment.replay().to_vec();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if pending.windows(QUERY.len()).any(|bytes| bytes == QUERY) {
                // A fresh headless screen starts at its first row and column.
                attachment
                    .send_input(b"\x1b[1;1R".to_vec())
                    .expect("the screen can answer its cursor query");
                return;
            }
            // A query may straddle output frames; retain only its possible prefix.
            let discard = pending.len().saturating_sub(QUERY.len() - 1);
            pending.drain(..discard);
            match attachment.next().await {
                Some(Event::Data(bytes)) => pending.extend_from_slice(&bytes),
                other => panic!("the terminal ended before its initial cursor query: {other:?}"),
            }
        }
    })
    .await
    .expect("the terminal did not send its initial cursor query");
}
