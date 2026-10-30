# orch-notifier-telegram

Telegram bot notification delivery to the people who linked a chat with the bot.

Messages are sent through the Bot API with the bot's token, which is never
written in an error or a log. A person is linked by a code carried in a link to
the bot and read back with `getUpdates`, only while a code waits, so nothing of
a system has to be reachable from outside; a person who stopped the bot or
deleted the chat is found at the next delivery and their route is switched off,
and the wait the Bot API asks for is kept. It is tested against a stand-in of
the Bot API on a local port.

| Module | Planned responsibility |
|--------|------------------------|
| [bot](src/bot.rs) | The Bot API as this notifier uses it: who the bot is, sending a message, and asking what the bot was sent; its limits, and the wait it is told to keep. |
| [linking](src/linking.rs) | A person linked to the bot: a link that carries a code, read back when they press Start; the bot is asked only while a code waits, so nothing of a system has to be reachable from outside. |
| [delivery](src/delivery.rs) | One notification, or several together, written to a person's chat within the length a message takes; a person who stopped the bot found at the next delivery and their route switched off. |
