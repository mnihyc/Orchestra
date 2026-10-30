# orch-notifier-email

SMTP notification delivery across configured security and authentication modes.

Mail through any server that speaks SMTP submission: implicit TLS, STARTTLS or
none, with or without signing in (plain or login, as the server offers), a
public or a private certificate authority, and nothing tuned to one provider. A
server is looked at when its channel is saved, and its own reply is the answer
to a failed test. STARTTLS that is asked for is never given up for plain text.
The name of a server is resolved here, and the connection goes to an address
that was looked at: a channel of an org or of a person is held to the private
hosts it may reach, the system's own is not. It is tested against a mail server
in a container, in every combination of security and signing in, and against a
scripted server for the replies that decide whether a message is tried again,
given up, or an address is gone.

| Module | Planned responsibility |
|--------|------------------------|
| [smtp](src/smtp.rs) | Configurable SMTP transport, authentication and TLS; and the check of a server when its channel is saved. |
| [render](src/render.rs) | A notification, or several together, as the subject and the two bodies of a mail, from the text its channel has for the recipient's language. |
| [delivery](src/delivery.rs) | Recipient delivery and failure outcomes: taken, to be retried, or an address that is gone. |
