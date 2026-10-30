//! Telegram bot notification delivery to the people who linked a chat with the bot.
//!
//! The notifier of a Telegram bot, which writes to the people who started a
//! chat with it. Three methods of the Bot API are used: `getMe`, when the
//! token is saved, to read the bot's name; `sendMessage`, to write a message as
//! plain text of at most 4 096 characters; and `getUpdates`, to read who pressed
//! Start on a link, only while a code waits for that. Nothing listens for
//! Telegram, so a system behind a firewall links people to its bot all the
//! same, and the only address it reaches is `api.telegram.org`.
//!
//! A person who blocked the bot is found at the next delivery, which Telegram
//! refuses for good: the notifier says the chat is gone, and the worker
//! switches the person's route off. A bot that asks to slow down is waited for
//! as long as it names.

#![forbid(unsafe_code)]

pub mod bot;
pub mod delivery;
pub mod linking;

pub use delivery::{NAME, TelegramNotifier, descriptor};
