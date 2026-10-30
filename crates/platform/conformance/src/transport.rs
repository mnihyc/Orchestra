//! The plugin transport's own cases: handshake, unknown members and methods, overlap, cancel and ping.
//!
//! Every external implementation speaks the same transport, whatever its kind (`orch-plugin-contract`): lines of
//! JSON, a handshake first, requests that may overlap and are matched by their ids, `cancel`, `ping` and
//! `shutdown`, members and methods it does not know tolerated. These cases check a plugin against those rules on
//! a link of their own, line by line, before anything of its kind is asked of it:
//!
//! | Rule | The plugin |
//! |------|------------|
//! | `transport.hello` | answers `hello` in time, with its own kind, versions inside those it was offered, its name and every method its kind requires |
//! | `transport.descriptor` | declares a descriptor that its kind's contract can read and that is consistent with itself |
//! | `transport.ping` | answers `ping` within ten seconds |
//! | `transport.unknown_member` | answers a request that carries members it does not know as if they were not there |
//! | `transport.unknown_method` | answers a method no contract has with the error `unsupported`, and goes on |
//! | `transport.overlap` | answers two requests sent before either was answered, each by its own id |
//! | `transport.cancel` | after `cancel`, answers the request it names (with its result or `cancelled`) and goes on |
//! | `transport.lines` | writes nothing that is not a message |
//! | `transport.shutdown` | a program that was started finishes and exits within ten seconds of `shutdown` |
//!
//! [`discover`] asks a plugin who it is before its kind is known, which is how `orchctl plugin check` finds the
//! suite to run: a plugin answers `hello` with its own kind whatever kind it was offered as.

use std::collections::BTreeMap;
use std::time::Duration;

use orch_plugin::negotiation::{check_descriptor, hello_deadline};
use orch_plugin::transport::{self, Line, OpenLink};
use orch_plugin_contract::lifecycle::{PING_REPLY_WITHIN, SHUTDOWN_WITHIN};
use orch_plugin_contract::messages::{
    FromPlugin, HELLO_ID, PluginError, RequestId, ToPlugin, code, method,
};
use orch_plugin_contract::negotiation::{HelloReply, Negotiated, accept, offer};
use orch_plugin_contract::registration::{PluginKind, PluginStart};
use orch_types::Json;
use serde_json::json;
use tokio::io::AsyncWriteExt;

use crate::report::{Checked, Report, skip};

/// A method that no contract has: a plugin answers it with `unsupported` and goes on.
pub const UNKNOWN_METHOD: &str = "x_conformance_no_such_method";

/// A member that no message has: a plugin reads past it.
const UNKNOWN_MEMBER: &str = "x_conformance_unknown";

/// How long a case that asks for one ordinary answer waits: the time a ping may take, which is the shortest
/// answer the transport asks for.
const ANSWER_WITHIN: Duration = PING_REPLY_WITHIN;

/// What a plugin answered to one request.
type Answer = Result<Json, PluginError>;

/// A link to a plugin, opened for the suite: lines are written and read one by one, and an answer that comes
/// while another is waited for is kept until it is asked for.
pub struct Link {
    open: OpenLink,
    next_id: RequestId,
    kept: BTreeMap<RequestId, Answer>,
    malformed: u64,
    started: bool,
}

impl Link {
    /// Starts the program, or attaches to the one that runs, as `start` says. What a started program writes to
    /// its standard error is read and let go of, so that a program that writes much there never blocks.
    pub async fn open(start: &PluginStart) -> Result<Self, String> {
        let mut open = transport::open(start)
            .await
            .map_err(|error| error.to_string())?;
        if let Some(mut log) = open.log.take() {
            tokio::spawn(async move {
                let mut sink = tokio::io::sink();
                // What the program logs is its own; the suite judges what it answers.
                let _ = tokio::io::copy(&mut log, &mut sink).await;
            });
        }
        Ok(Self {
            started: open.process.is_some(),
            open,
            next_id: HELLO_ID + 1,
            kept: BTreeMap::new(),
            malformed: 0,
        })
    }

    /// Writes one line.
    pub async fn send(&mut self, line: &str) -> Result<(), String> {
        let writer = &mut self.open.writer;
        let written = async {
            writer.write_all(line.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await
        };
        written
            .await
            .map_err(|error| format!("a line could not be written to it: {error}"))
    }

    /// Sends a request for `method` and returns its id.
    pub async fn request(&mut self, method: &str, params: Json) -> Result<RequestId, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&ToPlugin::request(id, method, params).to_line())
            .await?;
        Ok(id)
    }

    /// Takes the next id for a request that the caller writes itself.
    pub fn take_id(&mut self) -> RequestId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Waits for the answer to the request `id`, at most `within`. Progress and events that come meanwhile are
    /// passed over, and answers to other requests are kept for when they are asked for.
    pub async fn answer(&mut self, id: RequestId, within: Duration) -> Result<Answer, String> {
        if let Some(answer) = self.kept.remove(&id) {
            return Ok(answer);
        }
        let waited = tokio::time::timeout(within, async {
            loop {
                let line = match self.open.reader.next().await {
                    Ok(Some(Line::Text(text))) => text,
                    Ok(Some(Line::Oversized | Line::NotUtf8)) => {
                        self.malformed += 1;
                        continue;
                    }
                    Ok(None) => return Err("it closed its output".to_owned()),
                    Err(error) => return Err(format!("its output could not be read: {error}")),
                };
                if line.trim().is_empty() {
                    continue;
                }
                match FromPlugin::parse(&line) {
                    Ok(FromPlugin::Reply(reply)) if reply.id == id => return Ok(reply.outcome),
                    Ok(FromPlugin::Reply(reply)) => {
                        self.kept.insert(reply.id, reply.outcome);
                    }
                    Ok(FromPlugin::Progress(_) | FromPlugin::Event(_)) => {}
                    Err(_) => self.malformed += 1,
                }
            }
        })
        .await;
        waited.unwrap_or_else(|_| Err(format!("request {id} was not answered within {within:?}")))
    }

    /// How many lines it wrote that were no message.
    #[must_use]
    pub fn malformed(&self) -> u64 {
        self.malformed
    }

    /// Whether the suite started the program, rather than attaching to one that runs.
    #[must_use]
    pub fn started(&self) -> bool {
        self.started
    }

    /// Tells the plugin to finish and waits for a started program to exit, at most `within`. Returns whether it
    /// exited in time; one that did not is ended. An attached plugin is only let go of.
    pub async fn shut_down(mut self, within: Duration) -> bool {
        let _ = self.send(&ToPlugin::Shutdown.to_line()).await;
        let Some(mut process) = self.open.process.take() else {
            return true;
        };
        // A program that waits for its input to end before it exits is not held by a host that keeps it open.
        drop(self.open.writer);
        if tokio::time::timeout(within, process.wait()).await.is_ok() {
            true
        } else {
            process.kill().await;
            false
        }
    }
}

/// Asks the plugin who it is: opens a link, says `hello` as for a driver, and returns its answer, whose `kind`
/// is its own whatever it was offered as. The link is closed again before this returns.
pub async fn discover(start: &PluginStart, host_version: &str) -> Result<HelloReply, String> {
    let mut link = Link::open(start).await?;
    let hello = offer(PluginKind::Driver, host_version);
    let params = serde_json::to_value(&hello).map_err(|error| error.to_string())?;
    let sent = link
        .send(&ToPlugin::request(HELLO_ID, method::HELLO, params).to_line())
        .await;
    let answer = match sent {
        Ok(()) => {
            link.answer(HELLO_ID, hello_deadline(PluginKind::Driver))
                .await
        }
        Err(error) => Err(error),
    };
    link.shut_down(SHUTDOWN_WITHIN).await;
    match answer? {
        Ok(result) => serde_json::from_value(result)
            .map_err(|error| format!("its answer to hello is not one: {error}")),
        Err(error) => Err(format!(
            "it answered hello with the error {}: {}; name its kind with --kind",
            error.code, error.message
        )),
    }
}

/// Runs the transport's cases against the plugin that `start` reaches, expected to be a `kind`, and writes them
/// into `report`. Returns what was negotiated when the handshake succeeded; the cases after a failed handshake
/// are skipped, since nothing else can be asked of a plugin that did not say who it is.
pub async fn run(
    start: &PluginStart,
    kind: PluginKind,
    host_version: &str,
    report: &mut Report,
) -> Option<Negotiated> {
    let mut link = match Link::open(start).await {
        Ok(link) => link,
        Err(error) => {
            report.checked(
                "transport.hello",
                HELLO_TITLE,
                Err(format!("it could not be started or attached to: {error}").into()),
            );
            return None;
        }
    };
    let hello = offer(kind, host_version);
    let negotiated = handshake(&mut link, &hello).await;
    let negotiated = match negotiated {
        Ok(negotiated) => {
            report.checked("transport.hello", HELLO_TITLE, Ok(()));
            negotiated
        }
        Err(why) => {
            report.checked("transport.hello", HELLO_TITLE, Err(why.into()));
            for (rule, title) in AFTER_HELLO {
                report.checked(rule, title, Err(skip("not asked: the handshake failed")));
            }
            link.shut_down(SHUTDOWN_WITHIN).await;
            return None;
        }
    };
    report.checked(
        "transport.descriptor",
        DESCRIPTOR_TITLE,
        check_descriptor(kind, &negotiated.descriptor).map_err(Into::into),
    );
    let checked = ping(&mut link).await;
    report.checked("transport.ping", PING_TITLE, checked);
    let checked = unknown_member(&mut link).await;
    report.checked("transport.unknown_member", UNKNOWN_MEMBER_TITLE, checked);
    let checked = unknown_method(&mut link).await;
    report.checked("transport.unknown_method", UNKNOWN_METHOD_TITLE, checked);
    let checked = overlap(&mut link).await;
    report.checked("transport.overlap", OVERLAP_TITLE, checked);
    let checked = cancel(&mut link).await;
    report.checked("transport.cancel", CANCEL_TITLE, checked);
    let malformed = link.malformed();
    report.checked(
        "transport.lines",
        LINES_TITLE,
        if malformed == 0 {
            Ok(())
        } else {
            Err(format!("{malformed} lines it wrote were no message").into())
        },
    );
    if link.started() {
        let exited = link.shut_down(SHUTDOWN_WITHIN).await;
        report.checked(
            "transport.shutdown",
            SHUTDOWN_TITLE,
            if exited {
                Ok(())
            } else {
                Err(format!(
                    "it was still running {SHUTDOWN_WITHIN:?} after shutdown, and was ended"
                )
                .into())
            },
        );
    } else {
        link.shut_down(SHUTDOWN_WITHIN).await;
        report.checked(
            "transport.shutdown",
            SHUTDOWN_TITLE,
            Err(skip(
                "it was attached to: a running service belongs to whoever runs it and is not told to stop",
            )),
        );
    }
    Some(negotiated)
}

const HELLO_TITLE: &str = "It answers hello in time with its kind, versions it was offered, its name and the methods its kind requires";
const DESCRIPTOR_TITLE: &str =
    "Its descriptor is one its kind's contract can read, and is consistent with itself";
const PING_TITLE: &str = "It answers ping within ten seconds";
const UNKNOWN_MEMBER_TITLE: &str =
    "It answers a request with members it does not know as if they were not there";
const UNKNOWN_METHOD_TITLE: &str =
    "It answers a method no contract has with the error unsupported, and goes on";
const OVERLAP_TITLE: &str =
    "It answers two requests sent before either was answered, each by its own id";
const CANCEL_TITLE: &str =
    "After cancel it answers the request named, with its result or cancelled, and goes on";
const LINES_TITLE: &str = "Everything it writes on its output is a message";
const SHUTDOWN_TITLE: &str = "A started program finishes and exits within ten seconds of shutdown";

/// The cases that need a handshake that succeeded.
const AFTER_HELLO: [(&str, &str); 8] = [
    ("transport.descriptor", DESCRIPTOR_TITLE),
    ("transport.ping", PING_TITLE),
    ("transport.unknown_member", UNKNOWN_MEMBER_TITLE),
    ("transport.unknown_method", UNKNOWN_METHOD_TITLE),
    ("transport.overlap", OVERLAP_TITLE),
    ("transport.cancel", CANCEL_TITLE),
    ("transport.lines", LINES_TITLE),
    ("transport.shutdown", SHUTDOWN_TITLE),
];

async fn handshake(
    link: &mut Link,
    hello: &orch_plugin_contract::negotiation::Hello,
) -> Result<Negotiated, String> {
    let params = serde_json::to_value(hello).map_err(|error| error.to_string())?;
    link.send(&ToPlugin::request(HELLO_ID, method::HELLO, params).to_line())
        .await?;
    match link.answer(HELLO_ID, hello_deadline(hello.kind)).await? {
        Ok(result) => {
            let reply: HelloReply = serde_json::from_value(result)
                .map_err(|error| format!("its answer to hello is not one: {error}"))?;
            accept(hello, reply).map_err(|refused| refused.to_string())
        }
        Err(error) => Err(format!(
            "it answered hello with the error {}: {}",
            error.code, error.message
        )),
    }
}

/// Whether one answer is a result, as a case wants it.
fn answered(answer: Answer, what: &str) -> Checked {
    match answer {
        Ok(_) => Ok(()),
        Err(error) => Err(format!(
            "{what} was answered with the error {}: {}",
            error.code, error.message
        )
        .into()),
    }
}

async fn ping(link: &mut Link) -> Checked {
    let id = link.request(method::PING, json!({})).await?;
    let answer = link.answer(id, ANSWER_WITHIN).await?;
    answered(answer, "ping")
}

async fn unknown_member(link: &mut Link) -> Checked {
    let id = link.take_id();
    let line = json!({
        "id": id,
        "method": method::PING,
        "params": { UNKNOWN_MEMBER: true },
        UNKNOWN_MEMBER: true,
    })
    .to_string();
    link.send(&line).await?;
    let answer = link.answer(id, ANSWER_WITHIN).await?;
    answered(answer, "a ping with members it does not know")
}

async fn unknown_method(link: &mut Link) -> Checked {
    let id = link.request(UNKNOWN_METHOD, json!({})).await?;
    match link.answer(id, ANSWER_WITHIN).await? {
        Err(error) if error.code == code::UNSUPPORTED => {}
        Err(error) => {
            return Err(format!(
                "it answered with the error {} where unsupported was due",
                error.code
            )
            .into());
        }
        Ok(_) => return Err("it answered a method no contract has as if it knew it".into()),
    }
    let id = link.request(method::PING, json!({})).await?;
    let answer = link.answer(id, ANSWER_WITHIN).await?;
    answered(answer, "the ping after it")
}

async fn overlap(link: &mut Link) -> Checked {
    let first = link.request(method::PING, json!({})).await?;
    let second = link.request(method::PING, json!({})).await?;
    let second_answer = link.answer(second, ANSWER_WITHIN).await?;
    let first_answer = link.answer(first, ANSWER_WITHIN).await?;
    answered(first_answer, "the first of two pings")?;
    answered(second_answer, "the second of two pings")
}

async fn cancel(link: &mut Link) -> Checked {
    let cancelled = link.request(method::PING, json!({})).await?;
    link.send(&ToPlugin::Cancel { id: cancelled }.to_line())
        .await?;
    match link.answer(cancelled, ANSWER_WITHIN).await? {
        Ok(_) => {}
        Err(error) if error.code == code::CANCELLED => {}
        Err(error) => {
            return Err(format!(
                "the cancelled request was answered with the error {}, neither its result nor cancelled",
                error.code
            )
            .into());
        }
    }
    let id = link.request(method::PING, json!({})).await?;
    let answer = link.answer(id, ANSWER_WITHIN).await?;
    answered(answer, "the ping after cancel")
}
