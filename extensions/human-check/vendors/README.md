# orch-human-check

The human-check backends: hCaptcha, and reCAPTCHA of Google Cloud Fraud Defense through siteverify and through the Assessments API.

Each backend asks its vendor about one token and reports what the vendor said as facts; it decides nothing (the decision
is `orch_human_check_contract::judge`). The one HTTP client of both follows no redirect, gives up after the time the
system's setting names, reads at most a mebibyte of answer, and never writes the address it called, which carries a key,
into an error.

| Module | Responsibility |
|--------|----------------|
| [transport](src/transport.rs) | The one way a backend calls a vendor, and its scripted stand-in for tests. |
| [hcaptcha](src/hcaptcha.rs) | hCaptcha's `siteverify`: the request, its answer, its error words. |
| [recaptcha](src/recaptcha.rs) | What both ways of asking Google share: the widget's hosts. |
| [recaptcha_siteverify](src/recaptcha_siteverify.rs) | `siteverify` with the site key and the secret key the console shows, for a score-based key. |
| [recaptcha_assessments](src/recaptcha_assessments.rs) | The Assessments API of Google Cloud Fraud Defense, with a project and its API key, the way Google recommends. |

A further backend, whether another vendor or another way of asking one, is one more module with the trait of the
contract and a line in [`builtin`](src/lib.rs); nothing else changes.
