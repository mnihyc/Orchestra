//! Sequenced socket observations shared only by the terminalization and history-scale targets.

use crate::realtime_support::{Client, Heard, PATIENCE, Rig};
use crate::thread_history_support::Proof;
use orch_types::Id;
use serde_json::Value;
use std::collections::BTreeMap;

/// A sequence-aware mirror with exact turn IDs; never infers the current turn from old rows.
pub struct Observed {
    pub seq: i64,
    pub thread: Value,
    pub turns: BTreeMap<String, Value>,
    pub items: BTreeMap<String, Value>,
    pub inputs: BTreeMap<String, Value>,
    pub max_frame: usize,
}

impl Observed {
    pub fn new(seq: i64) -> Self {
        Self {
            seq,
            thread: Value::Null,
            turns: BTreeMap::new(),
            items: BTreeMap::new(),
            inputs: BTreeMap::new(),
            max_frame: 0,
        }
    }

    pub fn take(&mut self, frame: &Value) -> Proof {
        self.max_frame = self.max_frame.max(serde_json::to_vec(frame)?.len());
        assert_ne!(
            frame["op"], "resync",
            "a continuously consumed fixture feed must remain incremental"
        );
        if frame["op"] != "sync" {
            return Ok(());
        }
        let prev = frame["prev"].as_i64().ok_or("socket prev absent")?;
        let seq = frame["seq"].as_i64().ok_or("socket seq absent")?;
        assert!(
            prev <= self.seq && seq > prev,
            "the thread feed has a hole or a nonadvancing frame"
        );
        if seq <= self.seq {
            return Ok(());
        }
        self.seq = seq;
        let data = &frame["data"];
        if data["thread"].is_object() {
            self.thread = data["thread"].clone();
        }
        for (field, target) in [
            ("turns", &mut self.turns),
            ("items", &mut self.items),
            ("inputs", &mut self.inputs),
        ] {
            for row in data[field].as_array().into_iter().flatten() {
                let id = row["id"]
                    .as_str()
                    .ok_or("socket row identity absent")?
                    .to_owned();
                if target
                    .get(&id)
                    .is_none_or(|old| old["seq_updated"].as_i64() <= row["seq_updated"].as_i64())
                {
                    target.insert(id, row.clone());
                }
            }
        }
        Ok(())
    }

    pub async fn terminal(
        &mut self,
        socket: &mut Client,
        rig: &Rig,
        thread: Id,
        turn: Id,
        state: &str,
    ) -> Proof {
        // Receiving server rows does not satisfy the client heartbeat contract.
        // Do not use ping(), which discards unrelated frames while awaiting pong.
        socket.keep_alive().await;
        let result=tokio::time::timeout(PATIENCE, async {
            loop {
                let settled = self.thread["state"] == "idle" || (self.thread["state"] == "blocked"
                    && self.thread["blocked"]["reason"] == "error"
                    && self.thread["blocked"]["detail"] == "The last turn ended, but its usage and notifications could not be saved. Check the agent profile or storage.");
                if settled && self.thread["current_turn_id"].is_null()
                    && self.turns.get(&turn.to_string()).is_some_and(|row| row["state"] == state) {
                    return Ok::<(),Box<dyn std::error::Error+Send+Sync>>(());
                }
                match socket.hear_within(PATIENCE).await {
                    Some(Heard::Frame(frame)) => self.take(&frame)?,
                    _ => return Err("the terminal transition did not arrive on the open socket".into()),
                }
            }
        }).await;
        match result {
            Ok(Ok(())) => Ok(()),
            outcome => {
                rig.native_diagnostics(thread, "terminal socket observation failed")
                    .await;
                Err(format!("terminal socket observation for {state}: {outcome:?}").into())
            }
        }
    }
}

pub async fn subscribe(rig: &Rig, thread: Id, since: i64) -> Proof<Client> {
    let mut socket = Client::signed_in(rig.server.addr, &rig.token).await;
    socket
        .sub(1, &format!("thread:{thread}"), Some(since))
        .await;
    let accepted = socket.expect("subscribed").await;
    assert_eq!(accepted["mode"], "delta");
    socket.expect("presence").await;
    Ok(socket)
}
