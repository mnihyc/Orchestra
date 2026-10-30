//! Subscriptions, inbox, preferences, channels with their texts, routes with what each has sent, destinations, outbox and delivery records.
//!
//! These are the rows the worker works on, and the rows the engine and the
//! requests of a person's settings write for it to find. The parts:
//!
//! - [`channels`]: the ways out of the application and the places a node is
//!   told at, with their texts, the secrets that belong to them and what
//!   their last delivery came to.
//! - [`routes`]: one person's switch for one channel, how it is linked, what
//!   still waits to be told through it and how far it has told.
//! - [`outbox`]: every delivery a notifier is to make, claimed under a row
//!   lock so that no two workers take one, and finished with its outcome.
//! - [`people`]: whom a message is for, the browsers that take push, the
//!   places a notification happened at, and the Inbox row a notification
//!   starts as.
//! - [`upkeep`]: what the worker's schedule removes or trims.
//! - [`inbox`]: a person's Inbox as the pages read it, what is marked read,
//!   and what the person chose about being notified.
//! - [`subscribers`]: who follows a place, the automatic row of whoever owns
//!   or writes in a thread, and the destinations set at or above a node.
//! - [`events`]: what the engine reads to tell an event, and the delivery to
//!   a node's destination, written once.
//! - [`pages`]: the ways out that reach a person, their routes, the
//!   destinations of a node and the browsers that take push, as the pages of
//!   settings list and change them.
//!
//! A message is told through the outbox as a [`outbox::Payload`]: the
//! notifications it carries, written as they were when the row was made, so
//! that a delivery never depends on rows that changed since.

pub mod channels;
pub mod events;
pub mod exceptions;
pub mod inbox;
pub mod operations;
pub mod outbox;
pub mod pages;
pub mod people;
pub mod routes;
pub mod subscribers;
pub mod upkeep;
