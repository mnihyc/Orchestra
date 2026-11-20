# orch-human-check-contract

HumanCheckProvider trait seam; hCaptcha and reCAPTCHA of Google Cloud Fraud Defense are the first backends.

A human check is the proof a person gives, before anything is made or mailed, that they are not a program. The
backend ([`HumanCheckProvider`](src/lib.rs)) does only what differs between vendors: it asks the vendor about one
token and says what the vendor said ([`Facts`]), or that the vendor could not be asked ([`Unreachable`]). The
decision is made once, here, for every backend ([`judge`]): whether the token was for this act, was solved on this
site, and scored at least what the system asks. The API owns the rest: which acts are guarded, where the secret is
kept, the counts that bound the calls, and what is recorded.

Tests walk the decision with a scripted backend and the literal answers of each situation: a token that passes, one the
vendor refuses, one that is old, one that is used twice, one made for another act or another site, a score below the
setting, and a vendor that cannot be reached.
