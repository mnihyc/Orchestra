//! Database and outside-world delivery and upkeep; no machine orchestration.
//!
//! The worker needs only the database and the outside world. It delivers what
//! the engine and the requests of people write into the outbox, through the
//! notifier of each channel; it holds what a person asked to wait (an unread
//! delay, a limit on how often, quiet hours) and sends it as one message when
//! its time has come; it asks a bot who followed a link, only while a code
//! waits; and it removes what has run out. It never talks to a machine and
//! never runs engine logic: that is the API's.
//!
//! The library knows the contract of a notifier and nothing of any one of them:
//! the program that composes the worker registers the notifiers there are
//! ([`channels::Notifiers`]). Each scheduled job is a function here, run under
//! a lock of its own in the database ([`schedule`]) so that however many
//! workers are started, one runs it at a time.
//!
//! | Module | What it does |
//! |--------|--------------|
//! | [`outbox`] | Claims rows and delivers them, with retry |
//! | [`routes`] | The routes that wait, and what a person does to their own route |
//! | [`bot_links`] | Links a person to a bot by a code |
//! | [`channels`] | The kinds of channel, their secrets, and how one is saved and tried |
//! | [`channel_setup`] | A channel of the system set up from its fields on the host, and a sample sent through it |
//! | [`render`] | A message as a notifier gets it |
//! | [`system_mail`] | The mail that concerns a person's own user |
//! | [`quiet_hours`] | The windows in which nothing is sent |
//! | [`schedule`] | The jobs, their intervals and their locks |
//! | [`purge`], [`retention`] | What is removed or trimmed |
//! | [`exports`] | Archives of a place removed when their time has run out |
//! | [`command_purge`] | Commands to machines removed a day after they ended |
//! | [`heartbeat`] | Health and measurements |
//! | [`reconciliation`] | What a start brings back |
//! | [`reminders`] | Certificates about to end |
//! | [`spans`] | Days, hours and seconds of the configuration, bounded |
//! | [`catalogue`] | The rate catalogue fetched from the address an admin set, when it is due, and checked whole |
//! | [`rollup`] | The fold of the minutes of metric samples into hours that are kept |
//! | [`provider_renewals`] | The stopped runtimes of providers that are started so that their harness renews its sign-in |

#![forbid(unsafe_code)]

pub mod backups;
pub mod bot_links;
pub mod catalogue;
pub mod channel_setup;
pub mod channels;
pub mod command_purge;
pub mod document_purge;
pub mod document_versions;
pub mod exports;
pub mod gauges;
pub mod heartbeat;
pub mod outbox;
pub mod provider_renewals;
pub mod purge;
pub mod quiet_hours;
pub mod reconciliation;
pub mod releases;
pub mod reminders;
pub mod render;
pub mod retention;
pub mod rollup;
pub mod routes;
pub mod schedule;
pub mod side_threads;
pub mod spans;
pub mod system_mail;
