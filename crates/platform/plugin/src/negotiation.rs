//! Version and method negotiation.
//!
//! The first exchange with a plugin is always the same: the host sends `hello` as request 0, offering the
//! versions of the transport and of its kind's contract that it speaks, and the plugin answers with the ones it
//! will speak, who it is, the methods it implements and its descriptor. [`Handshake`] does that on the raw link,
//! before the session exists, and then holds the answer to everything the contract demands of it: the kind is the
//! one expected, the versions are inside what was offered, the plugin names itself, every required method is
//! there. Methods and members the host does not know are tolerated. Last, the descriptor is read as its kind's
//! descriptor and checked for consistency with itself ([`check_descriptor`]), so that a plugin whose declaration
//! cannot be used is stopped here, with the reason, and not found out by the screens that are drawn from it.
//!
//! What goes wrong is of two kinds, and they are treated differently by the supervisor. A plugin that does not
//! answer, or goes away while it is asked, has failed like any program ([`Failure::Silent`], [`Failure::Gone`]) and is started
//! again as its registration says. A plugin that answers and is refused ([`Failure::Refused`]) would answer the
//! same next time: it is stopped for good, and the reason is what an operator reads in Management.

use std::time::Duration;

use orch_agent_contract::descriptor::AgentDescriptor;
use orch_driver_contract::descriptor::DriverDescriptor;
use orch_executor_contract::descriptor::ExecutorDescriptor;
use orch_model_contract::translator::TranslatorDescriptor;
use orch_notifier_contract::descriptor::NotifierDescriptor;
use orch_plugin_contract::PluginKind;
use orch_plugin_contract::lifecycle::deadline;
use orch_plugin_contract::messages::{FromPlugin, HELLO_ID, Reply, ToPlugin, method};
use orch_plugin_contract::negotiation::{HandshakeError, HelloReply, Negotiated, accept, offer};
use orch_summarizer_contract::descriptor::SummarizerDescriptor;
use orch_types::{Clock, Json, Time};
use serde::de::DeserializeOwned;
use tokio::io::AsyncWriteExt;

use crate::requests::Counters;
use crate::transport::{Line, OpenLink};

/// Why a plugin that answered `hello` was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// Its answer breaks a rule of the handshake: another kind, a version that was not offered, no name, a
    /// required method missing.
    #[error("{0}")]
    Rejected(HandshakeError),
    /// It answered with an error, or with something that is not an answer to `hello`.
    #[error("{0}")]
    Answer(String),
    /// Its descriptor cannot be used.
    #[error("its descriptor cannot be used: {0}")]
    Descriptor(String),
}

/// Why no plugin came out of a handshake.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Failure {
    /// The plugin answered and was refused: it is stopped for good.
    #[error("{0}")]
    Refused(Refusal),
    /// The plugin went away before it answered: it closed its output, or could not be written to.
    #[error("{0}")]
    Gone(String),
    /// The plugin said nothing in time.
    #[error("{0}")]
    Silent(String),
}

/// How long `hello` may wait for its answer: the deadline of any method that states none.
#[must_use]
pub fn hello_deadline(kind: PluginKind) -> Duration {
    deadline(kind, method::HELLO)
}

/// What a handshake is done with: whom it is with, what is offered, and how long the answer may take.
pub struct Handshake<'a> {
    /// The plugin's name, for the log.
    pub plugin: &'a str,
    /// The kind the host expects.
    pub kind: PluginKind,
    /// The version of the host, which `hello` carries.
    pub host_version: &'a str,
    /// Where the time comes from.
    pub clock: &'a dyn Clock,
    /// How often the wait looks at the clock.
    pub tick: Duration,
    /// The moment the wait counts from: when the plugin was started or attached to.
    pub since: Time,
    /// How long the answer may take, by the clock, from then.
    pub within: Duration,
    /// Where lines that are no message are counted.
    pub counters: &'a Counters,
}

impl Handshake<'_> {
    /// Sends `hello` over `link` and holds the plugin's answer to the contract.
    ///
    /// Lines that are not the answer are skipped while it is waited for; those that are no message at all are
    /// counted.
    pub async fn run(&self, link: &mut OpenLink) -> Result<Negotiated, Failure> {
        run(self, link).await
    }
}

async fn run(this: &Handshake<'_>, link: &mut OpenLink) -> Result<Negotiated, Failure> {
    let Handshake {
        plugin,
        kind,
        host_version,
        clock,
        tick,
        since,
        within,
        counters,
    } = *this;
    let hello = offer(kind, host_version);
    let params = serde_json::to_value(&hello)
        .map_err(|error| Failure::Gone(format!("hello cannot be written: {error}")))?;
    let mut line = ToPlugin::request(HELLO_ID, method::HELLO, params).to_line();
    line.push('\n');
    let sent = async {
        link.writer.write_all(line.as_bytes()).await?;
        link.writer.flush().await
    };
    sent.await
        .map_err(|error| Failure::Gone(format!("hello could not be written: {error}")))?;

    let millis = |span: Duration| i64::try_from(span.as_millis()).unwrap_or(i64::MAX);
    let due = since.unix_millis().saturating_add(millis(within));
    let mut ticker = tokio::time::interval(tick);
    loop {
        tokio::select! {
            read = link.reader.next() => match read {
                Ok(Some(Line::Text(text))) => {
                    if text.trim().is_empty() {
                        continue;
                    }
                    match FromPlugin::parse(&text) {
                        Ok(FromPlugin::Reply(Reply { id: HELLO_ID, outcome })) => {
                            return decide(&hello, kind, outcome);
                        }
                        // Nothing else means anything before the plugin has said who it is.
                        Ok(_) => {}
                        Err(error) => counters.skipped_line(plugin, &error.to_string()),
                    }
                }
                Ok(Some(Line::Oversized)) => counters.skipped_line(plugin, "a line over the bound"),
                Ok(Some(Line::NotUtf8)) => counters.skipped_line(plugin, "a line that is not text"),
                Ok(None) => {
                    return Err(Failure::Gone(
                        "the plugin closed its output before it answered hello".to_owned(),
                    ));
                }
                Err(error) => {
                    return Err(Failure::Gone(format!(
                        "the plugin could not be read before it answered hello: {error}"
                    )));
                }
            },
            _ = ticker.tick() => {
                if clock.time().unix_millis() >= due {
                    return Err(Failure::Silent(format!(
                        "the plugin did not answer hello within {} s",
                        within.as_secs()
                    )));
                }
            }
        }
    }
}

fn decide(
    hello: &orch_plugin_contract::negotiation::Hello,
    kind: PluginKind,
    outcome: Result<Json, orch_plugin_contract::messages::PluginError>,
) -> Result<Negotiated, Failure> {
    let refused = |refusal| Failure::Refused(refusal);
    let result = outcome.map_err(|error| {
        refused(Refusal::Answer(format!(
            "the plugin answered hello with the error {}: {}",
            error.code, error.message
        )))
    })?;
    let reply: HelloReply = serde_json::from_value(result).map_err(|error| {
        refused(Refusal::Answer(format!(
            "its answer to hello is not the shape the contract states: {error}"
        )))
    })?;
    let negotiated = accept(hello, reply).map_err(|error| refused(Refusal::Rejected(error)))?;
    check_descriptor(kind, &negotiated.descriptor)
        .map_err(|fault| refused(Refusal::Descriptor(fault)))?;
    if kind == PluginKind::Provider {
        let descriptor =
            read::<orch_provider_contract::source::SourceDescriptor>(&negotiated.descriptor)
                .map_err(|fault| refused(Refusal::Descriptor(fault)))?;
        for method in orch_provider_contract::wire::METHODS {
            if descriptor.implements(method) && !negotiated.methods.supports(method) {
                return Err(refused(Refusal::Descriptor(format!(
                    "the source declares {method} but did not offer that method"
                ))));
            }
        }
    }
    Ok(negotiated)
}

fn read<T: DeserializeOwned>(descriptor: &Json) -> Result<T, String> {
    serde_json::from_value(descriptor.clone())
        .map_err(|error| format!("it is not the descriptor of its kind: {error}"))
}

fn faults<T: std::fmt::Display>(faults: &[T]) -> String {
    faults
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Reads `descriptor` as the descriptor of `kind` and checks it for consistency with itself, as the kind's
/// contract does for a built-in implementation. Members that belong to the implementation (`x_…`) are kept, and
/// members that are not known are ignored.
pub fn check_descriptor(kind: PluginKind, descriptor: &Json) -> Result<(), String> {
    match kind {
        PluginKind::Driver => {
            let driver = read::<DriverDescriptor>(descriptor)?;
            driver.check().map_err(|found| faults(&found))?;
            for schema in driver
                .spec_schemas
                .values()
                .chain(driver.capabilities.values())
            {
                crate::schema::check(schema).map_err(|error| error.to_string())?;
            }
            Ok(())
        }
        PluginKind::Adapter => {
            let agent = read::<AgentDescriptor>(descriptor)?;
            agent.check().map_err(|found| faults(&found))?;
            if let Some(provider) = agent.native_provider
                && !provider.options_schema.is_null()
            {
                crate::schema::check(&provider.options_schema)
                    .map_err(|error| error.to_string())?;
            }
            Ok(())
        }
        PluginKind::Executor => read::<ExecutorDescriptor>(descriptor)?
            .check()
            .map_err(|found| faults(&found)),
        PluginKind::Summarizer => {
            let summarizer = read::<SummarizerDescriptor>(descriptor)?;
            summarizer.check().map_err(|found| faults(&found))?;
            crate::schema::check(&summarizer.config_schema).map_err(|error| error.to_string())
        }
        PluginKind::Notifier => read::<NotifierDescriptor>(descriptor)?
            .check()
            .map_err(|found| faults(&found)),
        PluginKind::Provider => {
            read::<orch_provider_contract::source::SourceDescriptor>(descriptor)?
                .check()
                .map_err(|fault| fault.to_string())
        }
        PluginKind::Translator => {
            let translator = read::<TranslatorDescriptor>(descriptor)?;
            if translator.from.is_empty() || translator.to.is_empty() {
                Err(
                    "a translator names the protocol it reads and the protocol it writes"
                        .to_owned(),
                )
            } else if translator.from == translator.to {
                Err("a translator converts between two protocols, and these are one".to_owned())
            } else {
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::SystemClock;
    use serde_json::json;
    use tokio::io::{AsyncBufReadExt, AsyncWrite, BufReader, DuplexStream};

    use crate::transport::LineReader;

    /// The plugin's end of an in-memory link, and the host's end as the supervisor opens it.
    fn link() -> (OpenLink, BufReader<DuplexStream>, DuplexStream) {
        let (host_read, plugin_write) = tokio::io::duplex(64 * 1024);
        let (plugin_read, host_write) = tokio::io::duplex(64 * 1024);
        let link = OpenLink {
            reader: LineReader::new(host_read, 1024 * 1024),
            writer: Box::new(host_write) as Box<dyn AsyncWrite + Send + Unpin>,
            process: None,
            log: None,
        };
        (link, BufReader::new(plugin_read), plugin_write)
    }

    fn summarizer() -> Json {
        json!({ "name": "rules", "title": "By rules", "config_schema": {}, "fields": ["description"] })
    }

    fn reply(
        kind: &str,
        transport: u32,
        contract: u32,
        methods: &[&str],
        descriptor: Json,
    ) -> String {
        let result = json!({
            "kind": kind, "transport": transport, "contract": contract, "name": "rules", "version": "0.1.0",
            "methods": methods, "descriptor": descriptor
        });
        format!("{}\n", json!({ "id": 0, "result": result }))
    }

    async fn negotiate(answer: String) -> Result<Negotiated, Failure> {
        let (mut link, mut plugin_in, mut plugin_out) = link();
        let counters = Counters::default();
        let plugin = async {
            let mut hello = String::new();
            plugin_in.read_line(&mut hello).await.unwrap();
            let hello: Json = serde_json::from_str(&hello).unwrap();
            assert_eq!(hello["id"], 0);
            assert_eq!(hello["method"], "hello");
            assert_eq!(hello["params"]["kind"], "summarizer");
            assert_eq!(hello["params"]["transport"], json!([1, 1]));
            assert_eq!(hello["params"]["contract"], json!([1, 1]));
            plugin_out.write_all(answer.as_bytes()).await.unwrap();
        };
        let handshake = Handshake {
            plugin: "rules",
            kind: PluginKind::Summarizer,
            host_version: "0.1.0",
            clock: &SystemClock,
            tick: Duration::from_millis(5),
            since: SystemClock.time(),
            within: Duration::from_secs(30),
            counters: &counters,
        };
        let (negotiated, ()) = tokio::join!(handshake.run(&mut link), plugin);
        negotiated
    }

    #[tokio::test]
    async fn a_plugin_that_speaks_the_offered_versions_and_has_what_its_kind_requires_is_accepted()
    {
        let negotiated = negotiate(reply(
            "summarizer",
            1,
            1,
            &["describe", "summarize", "x_extra"],
            summarizer(),
        ))
        .await
        .unwrap();
        assert_eq!((negotiated.transport, negotiated.contract), (1, 1));
        assert_eq!(negotiated.identity.name, "rules");
        assert!(negotiated.methods.supports("summarize"));
        assert!(negotiated.methods.declared().any(|name| name == "x_extra"));
    }

    #[tokio::test]
    async fn a_version_below_or_above_the_offered_range_is_refused_for_the_transport_and_for_the_contract()
     {
        for (transport, contract) in [(0, 1), (2, 1), (1, 0), (1, 2)] {
            let refused = negotiate(reply(
                "summarizer",
                transport,
                contract,
                &["describe", "summarize"],
                summarizer(),
            ))
            .await
            .unwrap_err();
            let Failure::Refused(Refusal::Rejected(error)) = refused else {
                panic!("refused, for {transport} and {contract}");
            };
            let text = error.to_string();
            assert!(
                text.contains("[1, 1]"),
                "the versions the host offers are in the reason: {text}"
            );
            assert!(text.contains(&if transport == 1 { contract } else { transport }.to_string()));
        }
    }

    #[tokio::test]
    async fn another_kind_and_a_missing_required_method_are_refused_and_an_optional_one_is_not_needed()
     {
        let other = negotiate(reply(
            "notifier",
            1,
            1,
            &["describe", "summarize"],
            summarizer(),
        ))
        .await
        .unwrap_err();
        assert!(matches!(
            other,
            Failure::Refused(Refusal::Rejected(HandshakeError::KindMismatch { .. }))
        ));
        let missing = negotiate(reply("summarizer", 1, 1, &["describe"], summarizer()))
            .await
            .unwrap_err();
        assert_eq!(
            missing,
            Failure::Refused(Refusal::Rejected(HandshakeError::MissingRequired(vec![
                "summarize".into()
            ])))
        );
    }

    #[tokio::test]
    async fn a_descriptor_that_cannot_hold_together_stops_the_plugin_with_the_reason() {
        let mut broken = summarizer();
        broken["fields"] = json!([]);
        let refused = negotiate(reply(
            "summarizer",
            1,
            1,
            &["describe", "summarize"],
            broken,
        ))
        .await
        .unwrap_err();
        let Failure::Refused(Refusal::Descriptor(reason)) = refused else {
            panic!("a descriptor fault");
        };
        assert!(reason.contains("at least one field"), "{reason}");
        let not_a_descriptor = negotiate(reply(
            "summarizer",
            1,
            1,
            &["describe", "summarize"],
            json!("rules"),
        ))
        .await
        .unwrap_err();
        assert!(matches!(
            not_a_descriptor,
            Failure::Refused(Refusal::Descriptor(_))
        ));
    }

    #[tokio::test]
    async fn an_error_for_an_answer_and_a_plugin_that_goes_away_are_told_apart() {
        let error = negotiate(format!(
            "{}\n",
            json!({ "id": 0, "error": { "code": "unsupported_version", "message": "I speak 3 to 4" } })
        ))
        .await
        .unwrap_err();
        let Failure::Refused(Refusal::Answer(text)) = error else {
            panic!("an answer that is an error");
        };
        assert!(text.contains("I speak 3 to 4"));
        let silent_then_closed = {
            let (mut link, plugin_in, plugin_out) = link();
            drop((plugin_in, plugin_out));
            let counters = Counters::default();
            Handshake {
                plugin: "rules",
                kind: PluginKind::Summarizer,
                host_version: "0.1.0",
                clock: &SystemClock,
                tick: Duration::from_millis(5),
                since: SystemClock.time(),
                within: Duration::from_secs(30),
                counters: &counters,
            }
            .run(&mut link)
            .await
            .unwrap_err()
        };
        assert!(matches!(silent_then_closed, Failure::Gone(_)));
    }

    #[tokio::test]
    async fn a_plugin_that_does_not_answer_in_time_is_silent_and_lines_before_the_answer_are_skipped()
     {
        let clock = orch_types::ManualClock::new(chrono::Utc::now());
        let (mut link, mut plugin_in, mut plugin_out) = link();
        let counters = Counters::default();
        let driving = async {
            let mut hello = String::new();
            plugin_in.read_line(&mut hello).await.unwrap();
            plugin_out
                .write_all(b"garbage\n{\"event\":\"early\",\"params\":{}}\n")
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(40)).await;
            clock.advance(chrono::Duration::seconds(61));
        };
        let handshake = Handshake {
            plugin: "rules",
            kind: PluginKind::Summarizer,
            host_version: "0.1.0",
            clock: &clock,
            tick: Duration::from_millis(5),
            since: clock.time(),
            within: Duration::from_secs(60),
            counters: &counters,
        };
        let (outcome, ()) = tokio::join!(handshake.run(&mut link), driving);
        let Failure::Silent(text) = outcome.unwrap_err() else {
            panic!("silent");
        };
        assert!(text.contains("60 s"), "{text}");
        assert_eq!(counters.malformed_lines(), 1);
    }

    #[test]
    fn each_kind_reads_its_own_descriptor_and_a_translator_converts_between_two_protocols() {
        assert_eq!(
            check_descriptor(PluginKind::Summarizer, &summarizer()),
            Ok(())
        );
        assert!(check_descriptor(PluginKind::Driver, &summarizer()).is_err());
        let translator = |from: &str, to: &str| json!({ "from": from, "to": to, "title": "t" });
        assert_eq!(
            check_descriptor(PluginKind::Translator, &translator("openai-chat", "beta")),
            Ok(())
        );
        assert!(check_descriptor(PluginKind::Translator, &translator("beta", "beta")).is_err());
        assert!(check_descriptor(PluginKind::Translator, &translator("", "beta")).is_err());
        assert_eq!(hello_deadline(PluginKind::Driver), Duration::from_secs(60));
    }

    #[test]
    fn an_invalid_or_external_configuration_schema_is_refused_during_negotiation() {
        let mut descriptor = summarizer();
        descriptor["config_schema"] = json!({"type":"unknown"});
        assert!(check_descriptor(PluginKind::Summarizer, &descriptor).is_err());
        descriptor["config_schema"] = json!({"$ref":"file:///private/credential"});
        let refused = check_descriptor(PluginKind::Summarizer, &descriptor).unwrap_err();
        assert!(!refused.contains("private/credential"));
        descriptor["config_schema"] = json!({"$defs":{"text":{"type":"string"}},"type":"object","properties":{"name":{"$ref":"#/$defs/text"}},"additionalProperties":false});
        assert!(check_descriptor(PluginKind::Summarizer, &descriptor).is_ok());
    }
}
