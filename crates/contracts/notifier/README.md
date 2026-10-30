# orch-notifier-contract

Notification channel contract hosted by the worker.

The contract of notifiers, the kinds of channel (which reaches a person through their route) and of destination (which a node is told at) that the worker hosts: the `Notifier` trait (describe, check, deliver, and for a kind that links people, make a link and ask who followed it), the descriptor each publishes (targets, owners, how it reaches a person and so what a route needs, body kind, the longest message, fields and shipped texts), the delivery the worker hands over with the notifications it was rendered from, and the rules around them: the retry schedule and what becomes of a delivery, what a check of a saved channel decides, template placeholders checked when saved and escaped for a text, a JSON or an HTML medium, the choice of a text by language, the cut of a long text at a line, and which private hosts a notifier may reach. Each rule is tested as a rule and the descriptors and bodies are round-tripped on literal examples; two test notifiers (one that links people by a code, one that does not) exercise the trait.

| Module | Planned responsibility |
|--------|------------------------|
| [notifier](src/notifier.rs) | What a kind of channel does: describe itself, check a channel when it is saved, deliver, and, for a kind that links people, make a link and ask who followed it. |
| [descriptor](src/descriptor.rs) | A kind's identity; whom it reaches (a person's address, their devices, a chat linked by a code, a place somebody configured); who may set one up and how many; the fields a channel or a destination is made from; and the texts it ships. |
| [delivery](src/delivery.rs) | What is delivered to which target, one notification or several together, and the outcome: taken, to be retried and when, or gone. |
| [templates](src/templates.rs) | A text with placeholders from a closed list, in two forms and by language: checked when it is saved, and escaped for where it goes. |
| [outbound](src/outbound.rs) | Where a notifier may connect: an address that is not a public one is refused unless listed. |
