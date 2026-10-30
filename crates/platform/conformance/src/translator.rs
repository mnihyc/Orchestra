//! The suite every translator passes: requests and streamed responses converted, declared losses, and who presents the credential.
//!
//! The caller supplies a conversion example in the two formats. Streaming is checked both whole and
//! one byte at a time; buffering and UTF-8 boundaries cannot change the resulting answer.

use bytes::Bytes;
use orch_model_contract::translator::{
    ProtocolTranslator, TranslatedRequest, UpstreamSide, presenter,
};
use serde::{Deserialize, Serialize};

use crate::report::{Checked, Report, skip};

/// A representative conversion, without a real provider credential.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fixture {
    /// The source request's path.
    pub path: String,
    /// The source request's JSON text.
    pub body: String,
    /// The endpoint of the disposable upstream.
    pub upstream: UpstreamSide,
    /// The complete expected converted request, including credential ownership.
    pub request: TranslatedRequest,
    /// A complete response in the upstream format.
    pub response: String,
    /// The corresponding complete response in the harness format.
    pub translated_response: String,
    /// Protocols for which the host, rather than the translator, presents credentials.
    pub implemented: Vec<String>,
}

/// Checks declarations, exact conversion, credential ownership and response chunk boundaries.
pub fn run(translator: &dyn ProtocolTranslator, fixture: Option<&Fixture>, report: &mut Report) {
    let descriptor = translator.describe();
    report.checked(
        "translator.descriptor",
        "Its two ends and readable losses agree with its declaration",
        {
            let losses = descriptor
                .losses
                .preserved
                .iter()
                .chain(&descriptor.losses.approximated)
                .chain(&descriptor.losses.dropped);
            if descriptor.from == translator.from()
                && descriptor.to == translator.to()
                && !descriptor.from.is_empty()
                && !descriptor.to.is_empty()
                && !descriptor.title.trim().is_empty()
                && losses.clone().all(|line| !line.trim().is_empty())
            {
                Ok(())
            } else {
                Err("the declaration has unnamed ends, a missing title or unreadable losses".into())
            }
        },
    );
    let Some(fixture) = fixture else {
        for rule in [
            "translator.request",
            "translator.credential",
            "translator.response",
        ] {
            report.checked(
                rule,
                "Its conversion is verified",
                Err(skip("supply a translator fixture in its two formats")),
            );
        }
        return;
    };
    let converted = translator.translate_request(
        &fixture.path,
        Bytes::from(fixture.body.clone()),
        &fixture.upstream,
    );
    report.checked(
        "translator.request",
        "The converted request keeps the example's exact meaning",
        {
            match &converted {
                Ok(request) if request == &fixture.request => Ok(()),
                Ok(_) => {
                    Err("the converted request differs from the supplied expected request".into())
                }
                Err(error) => Err(error.to_string().into()),
            }
        },
    );
    let implemented: Vec<&str> = fixture.implemented.iter().map(String::as_str).collect();
    report.checked(
        "translator.credential",
        "Exactly the declared upstream side presents the credential",
        {
            match &converted {
                Ok(request) => request
                    .check_for(presenter(translator.to(), &implemented))
                    .map_err(|error| error.to_string().into()),
                Err(_) => Err(skip("no request was converted")),
            }
        },
    );
    report.checked(
        "translator.response",
        "Whole and fragmented streams produce the same exact response",
        response_case(translator, fixture),
    );
}

fn response_case(translator: &dyn ProtocolTranslator, fixture: &Fixture) -> Checked {
    let mut whole = translator.response_translator();
    let mut all = whole
        .translate_chunk(Bytes::from(fixture.response.clone()))
        .map_err(|error| error.to_string())?
        .to_vec();
    all.extend_from_slice(&whole.finish().map_err(|error| error.to_string())?);
    let mut fragmented = translator.response_translator();
    let mut pieces = Vec::new();
    for byte in fixture.response.as_bytes() {
        pieces.extend_from_slice(
            &fragmented
                .translate_chunk(Bytes::copy_from_slice(std::slice::from_ref(byte)))
                .map_err(|error| error.to_string())?,
        );
    }
    pieces.extend_from_slice(&fragmented.finish().map_err(|error| error.to_string())?);
    if all == fixture.translated_response.as_bytes() && pieces == all {
        Ok(())
    } else {
        Err("translated response bytes depend on chunk boundaries or differ from the expected response".into())
    }
}
