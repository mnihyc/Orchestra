# Synthetic native control boundary

The registered published-protocol corpora are in `tests/fixtures/harness/codex/<version>` at the workspace root. Their full manifests, native startup handshakes and actor scripts are read by the existing upgrade_controls test through the common Corpus/replay_io helper.

Only background_control_declared_but_missing remains here: an explicitly synthetic 0.170.0 schema declares both terminal list and terminate, but the scripted native server returns method-not-found. It proves safe unavailable handling on a declared control. It is not a capability or recording claim for either pinned version. The source-version directories keep its two explicit test contexts; the native startup version is visibly synthetic in both scripts.

A build below the oldest proven one is refused at the start, so no startable build lacks the controls; a build that has changed them fails at the use of the act, which this case covers. The unmodified list and End of the shipped build, and Stop and close, live in the registered corpora and in `upgrade_tolerance.rs`.
