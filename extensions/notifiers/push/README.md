# orch-notifier-push

Web Push notification delivery.

Messages are encrypted for each browser's subscription and signed with the
deployment's VAPID key (a PEM or a base64 private value), whose public half is
what a browser subscribes with; they are sent through a client that follows no
redirect, and an endpoint, which a person supplies, is held to the rule about
private addresses even for the system's own channel. A subscription that a push
service reports as gone is removed. It is tested with a stand-in push service
that decrypts what it receives as a browser would and checks the signature.
The encryption-only dependency seals RFC 8291 messages; P-256 signs the VAPID
token directly, and the standard's example ciphertext is also opened in tests.

| Module | Planned responsibility |
|--------|------------------------|
| [vapid](src/vapid.rs) | Configured VAPID identity. |
| [encryption](src/encryption.rs) | Browser push encryption with a fresh sender key and salt. |
| [payload](src/payload.rs) | Push content and target URLs. |
| [delivery](src/delivery.rs) | Delivery to a person's browsers, and a subscription removed when it has expired. |
