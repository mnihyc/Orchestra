//! External ProtocolTranslator bridge.
//!
//! [`TranslatorBridge`] is a [`ProtocolTranslator`] whose implementation is a program. A translator converts the
//! requests of one wire protocol (`from`, the one the harness speaks) into another (`to`, the one the upstream
//! speaks), and the response stream back, chunk by chunk. None is built in: this is how one is installed.
//!
//! The trait is not asynchronous, so the bridge waits on the calling thread for each answer
//! ([`crate::requests::Call::wait_blocking`]). Call it from a thread that may block: on a multi-thread runtime
//! the thread is handed back while it waits.
//!
//! | Method | Required | Parameters | Result |
//! |--------|----------|------------|--------|
//! | `describe` | yes | none | the translator's descriptor |
//! | `translate_request` | yes | `path` below the upstream's base, `body` as text, `upstream`: its `base_url`, `options` and, for a translator that presents the credential itself, the `credential` as the key's text | the request to make of the upstream: `method`, `path`, `query`, `headers`, `body` as text, `authorized` |
//! | `response_translator` | yes | none | `converter`: a name for the converter that the program keeps for one response |
//! | `translate_chunk` | yes | `converter`, `chunk` as text | `chunk`: what the conversion yields so far, which may be empty or longer than what it was given |
//! | `finish_response` | yes | `converter` | `chunk`: whatever the conversion still holds, and the closing of the stream |
//!
//! The bodies of model APIs are text, and are sent as text. A chunk of a response may end in the middle of a
//! character; the bridge holds the beginning of it until the rest comes, so the program is only ever given whole
//! characters. A converter is opened when the first chunk (or the end) of its response comes, so a response that
//! never comes opens none, and a converter that is dropped before the response ended is finished for the
//! program, which can then let it go. The program answers `malformed` for a request that is none of the `from`
//! protocol, and `unsupported` for one with something that has no equivalent and that it will not drop.

use bytes::Bytes;
use orch_model_contract::translator::{
    ProtocolTranslator, ResponseTranslator, TranslateError, TranslatedRequest,
    TranslatorDescriptor, UpstreamSide,
};
use orch_plugin_contract::PluginKind;
use serde_json::json;

use super::{BridgeError, Described, interned, member, params, result};
use crate::requests::{Call, CallError};
use crate::supervisor::Plugin;

const TRANSLATE_REQUEST: &str = "translate_request";
const RESPONSE_TRANSLATOR: &str = "response_translator";
const TRANSLATE_CHUNK: &str = "translate_chunk";
const FINISH_RESPONSE: &str = "finish_response";

/// A translator that is a program.
pub struct TranslatorBridge {
    plugin: Plugin,
    descriptor: Described<TranslatorDescriptor>,
    from: &'static str,
    to: &'static str,
}

impl TranslatorBridge {
    /// The translator that `plugin` is. The plugin must be of the kind and have said hello.
    pub fn new(plugin: Plugin) -> Result<Self, BridgeError> {
        let descriptor =
            Described::<TranslatorDescriptor>::new(plugin.clone(), PluginKind::Translator)?;
        let described = descriptor.get();
        let (from, to) = (interned(&described.from), interned(&described.to));
        Ok(Self {
            plugin,
            descriptor,
            from,
            to,
        })
    }
}

/// The error of a conversion for what the program answered.
fn error_of(error: &CallError) -> TranslateError {
    match error {
        CallError::Failed(remote) => match remote.code.as_str() {
            "malformed" => TranslateError::Malformed(remote.message.clone()),
            "unsupported" => TranslateError::Unsupported(remote.message.clone()),
            other => TranslateError::Internal(format!("{other}: {}", remote.message)),
        },
        _ => TranslateError::Internal(error.to_string()),
    }
}

/// Asks the program `method`, and waits for its answer on this thread.
fn ask(
    plugin: &Plugin,
    method: &str,
    sent: &impl serde::Serialize,
) -> Result<serde_json::Value, TranslateError> {
    params(method, sent)
        .and_then(|sent| plugin.start_call(method, sent))
        .and_then(Call::wait_blocking)
        .map_err(|error| error_of(&error))
}

impl ProtocolTranslator for TranslatorBridge {
    fn from(&self) -> &'static str {
        self.from
    }

    fn to(&self) -> &'static str {
        self.to
    }

    fn describe(&self) -> TranslatorDescriptor {
        (*self.descriptor.get()).clone()
    }

    fn translate_request(
        &self,
        path: &str,
        body: Bytes,
        upstream: &UpstreamSide,
    ) -> Result<TranslatedRequest, TranslateError> {
        let text = std::str::from_utf8(&body).map_err(|error| {
            TranslateError::Malformed(format!("the request is not text: {error}"))
        })?;
        let answer = ask(
            &self.plugin,
            TRANSLATE_REQUEST,
            &json!({ "path": path, "body": text, "upstream": upstream }),
        )?;
        result(TRANSLATE_REQUEST, answer).map_err(|error| error_of(&error))
    }

    fn response_translator(&self) -> Box<dyn ResponseTranslator> {
        Box::new(ResponseBridge {
            plugin: self.plugin.clone(),
            converter: None,
            held: Vec::new(),
            finished: false,
        })
    }
}

/// The converter of one response, kept by the program.
struct ResponseBridge {
    plugin: Plugin,
    converter: Option<String>,
    /// The beginning of a character whose end has not come.
    held: Vec<u8>,
    finished: bool,
}

impl ResponseBridge {
    /// The name of the program's converter, which is asked for when it is first wanted.
    fn converter(&mut self) -> Result<String, TranslateError> {
        if let Some(converter) = &self.converter {
            return Ok(converter.clone());
        }
        let answer = ask(&self.plugin, RESPONSE_TRANSLATOR, &json!({}))?;
        let converter: String =
            member(RESPONSE_TRANSLATOR, answer, "converter").map_err(|error| error_of(&error))?;
        self.converter = Some(converter.clone());
        Ok(converter)
    }

    /// The whole characters of what has come so far; the beginning of one whose end is missing is held.
    fn text_of(&mut self, chunk: &[u8]) -> Result<String, TranslateError> {
        self.held.extend_from_slice(chunk);
        let valid = match std::str::from_utf8(&self.held) {
            Ok(_) => self.held.len(),
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => {
                return Err(TranslateError::Malformed(
                    "the response is not text".to_owned(),
                ));
            }
        };
        let rest = self.held.split_off(valid);
        let whole = std::mem::replace(&mut self.held, rest);
        String::from_utf8(whole)
            .map_err(|_| TranslateError::Malformed("the response is not text".to_owned()))
    }

    fn converted(
        &self,
        method: &str,
        converter: &str,
        chunk: Option<&str>,
    ) -> Result<Bytes, TranslateError> {
        let sent = match chunk {
            Some(chunk) => json!({ "converter": converter, "chunk": chunk }),
            None => json!({ "converter": converter }),
        };
        let answer = ask(&self.plugin, method, &sent)?;
        let text: String = member(method, answer, "chunk").map_err(|error| error_of(&error))?;
        Ok(Bytes::from(text))
    }
}

impl ResponseTranslator for ResponseBridge {
    fn translate_chunk(&mut self, chunk: Bytes) -> Result<Bytes, TranslateError> {
        let text = self.text_of(&chunk)?;
        if text.is_empty() {
            // Nothing but the beginning of a character yet.
            return Ok(Bytes::new());
        }
        let converter = self.converter()?;
        self.converted(TRANSLATE_CHUNK, &converter, Some(&text))
    }

    fn finish(&mut self) -> Result<Bytes, TranslateError> {
        self.finished = true;
        if !self.held.is_empty() {
            return Err(TranslateError::Malformed(
                "the response ended in the middle of a character".to_owned(),
            ));
        }
        let converter = self.converter()?;
        self.converted(FINISH_RESPONSE, &converter, None)
    }
}

impl Drop for ResponseBridge {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if let Some(converter) = &self.converter {
            // The response was given up: the program is told, so that it can let the converter go. Nobody waits.
            if let Ok(call) = self
                .plugin
                .start_call(FINISH_RESPONSE, json!({ "converter": converter }))
            {
                call.detach();
            }
        }
    }
}
