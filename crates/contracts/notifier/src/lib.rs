//! Notification channel contract hosted by the worker.
//!
//! A **notifier** is the kind of a channel, which reaches a person through that person's route, or of a
//! destination, which a node is told at. Mail, a Telegram bot, a webhook and Web Push are built in, and a
//! further one, an integration with a chat tool, is one more implementation of [`notifier::Notifier`]: its
//! fields, the texts it ships, and its way of linking people where it has to. The engine decides who is told
//! what; the worker renders the texts and hands the notifier a finished [`delivery::Delivery`]; the notifier
//! sends, and never reads the database.
//!
//! | Module | What it holds |
//! |--------|---------------|
//! | [`notifier`] | The trait |
//! | [`descriptor`] | What a notifier declares: whom it reaches, who may set one up, its fields and texts |
//! | [`delivery`] | A delivery, the notifications it carries, the errors, and what becomes of a delivery |
//! | [`templates`] | The texts with placeholders: checked when saved, escaped for where they go, cut to fit |
//! | [`outbound`] | Where a notifier may connect |

#![forbid(unsafe_code)]

pub mod delivery;
pub mod descriptor;
pub mod notifier;
pub mod outbound;
pub mod templates;
