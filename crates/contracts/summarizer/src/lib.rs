//! Summarizer contract for summary paragraphs, titles, tags and handoffs.
//!
//! A summarizer turns recent activity into bounded output. Written descriptions are separate from
//! automatic summary paragraphs, titles and tags. The engine owns when, how often, manual edits, and the
//! ledger; an implementation, built in or installed, only turns activity into text. This crate holds the
//! trait ([`summarizer::Summarizer`]), what a summarizer declares ([`descriptor`]), what it is shown
//! ([`activity`]) and what it returns, held to the one rule for what may be written ([`result`]).

#![forbid(unsafe_code)]

pub mod activity;
pub mod descriptor;
pub mod result;
pub mod summarizer;
