# Installation facts from released programs

Supply actual packaged releases with `ORCH_SMOKE_RELEASE_A` and `ORCH_SMOKE_RELEASE_B`, B newer, and
run `bash scripts/smoke/installation-records/run.sh`. It starts its own candidate stack and a local
configured manifest source on ports 41430–41432. It builds and installs no development tools.

The check ships off. An explicit request reads only the configured manifest, reports a newer
offer and one local notice, and leaves the installed version unchanged. A later invalid source
preserves the successful facts with their original date. The existing released backup command is
run while the API is running and stopped; startup imports its protected manifest-led metadata.
A wrong-installation record cannot replace the last success. Metadata repair leaves the count
and bytes of dumps unchanged and preserves the pair's original completion time and cause.

Logs, exact candidate manifest, API-visible facts and command exits survive cleanup under the
run's evidence directory. Tokens, TLS private keys, database contents and the private inventory
manifest are not copied into that evidence. API integration tests additionally cover off/due
checks, size/time/redirect/older-manifest refusal, restricted callers, manifest hashes, failed
attempts and the held-clock backup warning boundary. The scenario and tests are written but not
claimed as executed; the integrator supplies their real result.
