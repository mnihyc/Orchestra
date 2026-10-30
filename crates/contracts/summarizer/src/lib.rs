//! Summarizer contract for title, description and tag generation.
//!
//! A summarizer turns the recent activity of a node into the text of one of its fields: a title, a
//! description, or tags. The engine owns when, how often, the rule that a person's edit wins, and the
//! ledger; an implementation, built in or installed, only turns activity into text. This crate holds the
//! trait ([`summarizer::Summarizer`]), what a summarizer declares ([`descriptor`]), what it is shown
//! ([`activity`]) and what it returns, held to the one rule for what may be written ([`result`]).

#![forbid(unsafe_code)]

pub mod activity;
pub mod descriptor;
pub mod result;
pub mod summarizer;
