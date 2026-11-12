# Synthetic native control boundary

The registered published-protocol corpora are in `tests/fixtures/harness/codex/<version>` at the workspace root. Their full manifests, native startup handshakes and actor scripts are read by the existing upgrade_controls test through the common Corpus/replay_io helper.

Only background_control_declared_but_missing remains here: an explicitly synthetic 0.170.0 schema declares both terminal list and terminate, but the scripted native server returns method-not-found. It proves safe unavailable handling on a declared control. It is not a capability or recording claim for either pinned version. The source-version directories keep its two explicit test contexts; the native startup version is visibly synthetic in both scripts.

Both pinned schemas instead declare native End unoffered and never probe either method. Their negative capability, Stop and close cases live in the registered corpora.
