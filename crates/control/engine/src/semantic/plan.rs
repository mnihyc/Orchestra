//! What a change of the embeddings named for an org means for the vectors it has, and whether a new set fits.
//!
//! Vectors of different models, prefixes, ways of cutting or lengths cannot be compared, so such a change makes
//! a new generation of every vector. Two changes need none: the same model served by another endpoint or under
//! another name, which the stored probe texts show when they are made again and come out alike; and a shorter
//! vector of a model that keeps its meaning in its first numbers, whose stored vectors are cut, made unit length
//! again and indexed anew. A new generation is built beside the one that answers while the disk share holds
//! both; when it cannot, an admin chooses first, seeing the size and the time.

/// What a generation of vectors was made with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Made {
    /// The provider whose endpoint made them.
    pub provider_id: orch_types::Id,
    /// The model, by the name that was sent.
    pub model_name: String,
    /// The length that was asked for; `None` for the model's own.
    pub asked_dimensions: Option<u32>,
    /// What was put before a query.
    pub query_prefix: String,
    /// What was put before a document.
    pub document_prefix: String,
    /// How the text was cut.
    pub chunking: String,
}

/// What a change means for the vectors there are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Nothing the vectors depend on changed.
    Nothing,
    /// Another endpoint or another name of model: the probe texts tell whether it is the same model.
    Elsewhere,
    /// Shorter vectors, perhaps from another endpoint: if the probe texts agree, the stored vectors are cut to
    /// this length and only the index is made again.
    Shorter {
        /// The new length.
        to: u32,
    },
    /// Vectors that the stored ones cannot be compared with: a new generation.
    Rebuild,
}

/// What a change from `made` to `wanted` means.
#[must_use]
pub fn change(made: &Made, wanted: &Made) -> Change {
    if made.query_prefix != wanted.query_prefix
        || made.document_prefix != wanted.document_prefix
        || made.chunking != wanted.chunking
    {
        return Change::Rebuild;
    }
    let same_source =
        made.provider_id == wanted.provider_id && made.model_name == wanted.model_name;
    match (made.asked_dimensions, wanted.asked_dimensions) {
        (asked, wanted_length) if asked == wanted_length => {
            if same_source {
                Change::Nothing
            } else {
                Change::Elsewhere
            }
        }
        (Some(before), Some(after)) if after < before => Change::Shorter { to: after },
        // The model's own length is the longest it gives: from a shortened one, it is longer.
        (None, Some(after)) => Change::Shorter { to: after },
        _ => Change::Rebuild,
    }
}

/// The most bytes the index may take: `share` percent of the bytes it indexes; `None` is no limit.
#[must_use]
pub fn allowed(content: i64, share: Option<i64>) -> Option<i64> {
    share.map(|share| {
        let allowed = i128::from(content.max(0)) * i128::from(share.max(0)) / 100;
        i64::try_from(allowed).unwrap_or(i64::MAX)
    })
}

/// Whether `adding` more bytes keep what the index takes within its share of what it indexes. `share` is a
/// percentage; `None` is no limit.
#[must_use]
pub fn fits(used: i64, adding: i64, content: i64, share: Option<i64>) -> bool {
    allowed(content, share).is_none_or(|allowed| used.saturating_add(adding) <= allowed)
}

/// How long the pending tokens take at the measured rate, in whole seconds; `None` before a rate is known.
#[must_use]
pub fn seconds_left(pending_tokens: i64, rate: Option<f64>) -> Option<i64> {
    let rate = rate.filter(|rate| *rate > 0.0 && rate.is_finite())?;
    // Whole seconds, rounded up: an estimate never claims to be done early.
    Some((pending_tokens.max(0) as f64 / rate).ceil() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::Id;

    fn made() -> Made {
        Made {
            provider_id: Id::from_u128(1),
            model_name: "embeddinggemma-2".into(),
            asked_dimensions: Some(256),
            query_prefix: "task: search result | query: ".into(),
            document_prefix: "title: {title} | text: ".into(),
            chunking: "parts-v1".into(),
        }
    }

    #[test]
    fn the_same_model_elsewhere_and_a_shorter_vector_need_no_new_generation_but_other_changes_do() {
        let before = made();
        assert_eq!(change(&before, &made()), Change::Nothing);
        let elsewhere = Made {
            provider_id: Id::from_u128(2),
            ..made()
        };
        assert_eq!(change(&before, &elsewhere), Change::Elsewhere);
        let renamed = Made {
            model_name: "google/embeddinggemma-2".into(),
            ..made()
        };
        assert_eq!(change(&before, &renamed), Change::Elsewhere);
        let shorter = Made {
            asked_dimensions: Some(128),
            ..made()
        };
        assert_eq!(change(&before, &shorter), Change::Shorter { to: 128 });
        let full = Made {
            asked_dimensions: None,
            ..made()
        };
        assert_eq!(change(&full, &before), Change::Shorter { to: 256 });
        assert_eq!(
            change(&before, &full),
            Change::Rebuild,
            "longer vectors are made anew"
        );
        let longer = Made {
            asked_dimensions: Some(512),
            ..made()
        };
        assert_eq!(change(&before, &longer), Change::Rebuild);
        for other in [
            Made {
                query_prefix: String::new(),
                ..made()
            },
            Made {
                document_prefix: "passage: ".into(),
                ..made()
            },
            Made {
                chunking: "parts-v2".into(),
                ..made()
            },
        ] {
            assert_eq!(change(&before, &other), Change::Rebuild, "{other:?}");
        }
    }

    #[test]
    fn a_share_holds_up_to_its_percentage_of_the_content_and_no_share_holds_everything() {
        assert!(fits(40, 10, 100, Some(50)));
        assert!(!fits(40, 11, 100, Some(50)));
        assert!(fits(0, 0, 0, Some(100)));
        assert!(!fits(0, 1, 0, Some(100)));
        assert!(fits(i64::MAX, i64::MAX, 0, None));
        assert!(
            !fits(1, 0, 100, Some(0)),
            "a share of nothing holds nothing"
        );
    }

    #[test]
    fn the_time_left_rounds_up_and_is_unknown_without_a_rate() {
        assert_eq!(seconds_left(1_000, Some(212.0)), Some(5));
        assert_eq!(seconds_left(0, Some(212.0)), Some(0));
        assert_eq!(seconds_left(1_000, None), None);
        assert_eq!(seconds_left(1_000, Some(0.0)), None);
    }
}
