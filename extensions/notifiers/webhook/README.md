# orch-notifier-webhook

Signed webhook notification delivery.

A message is posted as JSON, signed with the secret of the channel or the
destination (HMAC-SHA256 in `X-Orch-Signature`), to an address that is checked
against the private networks at the moment of connecting and again at every
redirect, which is followed a few times at most. The answer decides the outcome:
taken, tried again (a wait the far side names is kept, within a day), refused,
or gone, which ends the destination. It is tested against stand-in servers on
local ports, including one that redirects to a private address.

| Module | Planned responsibility |
|--------|------------------------|
| [targets](src/targets.rs) | Target resolution and configured network restrictions. |
| [signature](src/signature.rs) | HMAC payload signatures. |
| [delivery](src/delivery.rs) | HTTP delivery, with a text of the channel's own rendered as the body where it has one, and retryable outcomes. |
