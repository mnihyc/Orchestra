//! What a model's tokenizer comes to, from the catalogue of what was measured, for the estimates where nobody counted.
//!
//! The catalogue (`data/token-density.json`) lists model families, each with the patterns of the routing names it
//! covers (a gateway's prefix is part of the name) and the measures: bytes of ASCII prose and of structured text per
//! token, characters beyond ASCII per token, and what an image takes. The first family a name matches gives its
//! measures; a model of no family listed is counted between the families that were measured. Nothing here knows a
//! vendor: a family is a row of the data.

use std::sync::OnceLock;

use globset::{Glob, GlobSet, GlobSetBuilder};
use orch_api_contract::programs::TokenEstimate;
use serde::Deserialize;

/// The embedded catalogue.
const CATALOGUE: &str = include_str!("../data/token-density.json");

#[derive(Debug, Deserialize)]
struct Measures {
    ascii_prose_x100: i64,
    ascii_structured_x100: i64,
    other_x100: i64,
    image_tokens: i64,
}

#[derive(Debug, Deserialize)]
struct Family {
    matches: Vec<String>,
    #[serde(flatten)]
    measures: Measures,
}

#[derive(Debug, Deserialize)]
struct Catalogue {
    families: Vec<Family>,
    unmeasured: Measures,
}

struct Loaded {
    families: Vec<(GlobSet, TokenEstimate)>,
    unmeasured: TokenEstimate,
}

fn estimate(measures: &Measures, measured: bool) -> TokenEstimate {
    TokenEstimate {
        ascii_prose_x100: measures.ascii_prose_x100,
        ascii_structured_x100: measures.ascii_structured_x100,
        other_x100: measures.other_x100,
        image_tokens: measures.image_tokens,
        measured,
    }
}

fn loaded() -> &'static Loaded {
    static LOADED: OnceLock<Loaded> = OnceLock::new();
    LOADED.get_or_init(|| {
        #[expect(
            clippy::expect_used,
            reason = "the catalogue is data of this package and a test of it reads every row"
        )]
        let catalogue: Catalogue =
            serde_json::from_str(CATALOGUE).expect("the built-in token catalogue is valid");
        let families = catalogue
            .families
            .iter()
            .map(|family| {
                let mut set = GlobSetBuilder::new();
                for pattern in &family.matches {
                    #[expect(
                        clippy::expect_used,
                        reason = "the patterns are data of this package and a test of it reads every row"
                    )]
                    set.add(Glob::new(pattern).expect("a pattern of the token catalogue is a glob"));
                }
                #[expect(
                    clippy::expect_used,
                    reason = "the patterns are data of this package and a test of it reads every row"
                )]
                let set = set.build().expect("the patterns of a family build");
                (set, estimate(&family.measures, true))
            })
            .collect();
        Loaded {
            families,
            unmeasured: estimate(&catalogue.unmeasured, false),
        }
    })
}

/// What the tokenizer of the model called `model` comes to, by its routing name, a gateway's prefix and all.
#[must_use]
pub fn of(model: &str) -> TokenEstimate {
    let name = model.to_ascii_lowercase();
    let loaded = loaded();
    loaded
        .families
        .iter()
        .find(|(patterns, _)| patterns.is_match(&name))
        .map_or(loaded.unmeasured, |(_, measures)| *measures)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_models_family_is_told_by_its_routing_name_through_a_gateway_too() {
        assert_eq!(of("gpt-6-sol").ascii_prose_x100, 482);
        assert_eq!(of("openai/gpt-5.6-luna").ascii_prose_x100, 482);
        assert_eq!(of("o4-mini").ascii_prose_x100, 482);
        assert_eq!(of("codex-auto-review").ascii_prose_x100, 482);
        assert_eq!(of("claude-sonnet-5[1m]").ascii_prose_x100, 331);
        assert_eq!(of("anthropic/claude-haiku-5.5").ascii_prose_x100, 331);
        assert!(of("claude-sonnet-5").measured);
        assert!(!of("qwen/qwen3.8-flash").measured);
        assert_eq!(of("qwen/qwen3.8-flash").ascii_prose_x100, 400);
        assert_eq!(
            of("ollama"),
            of("something-else-entirely"),
            "a name that begins with o and no digit is no o-series"
        );
        assert_eq!(
            of("Claude-Sonnet-5").other_x100,
            86,
            "names are read in lower case"
        );
    }
}
