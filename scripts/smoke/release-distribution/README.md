# Release distribution

`bash scripts/smoke/release-distribution/run.sh` uses the two supplied or packaged
releases, a fresh application stack, a loopback release site and an official
registry pinned by index digest. It installs the first release offline, fetches
the next one through the updater's actual site path, and proves the absent-image
pull by the updater's log and the consumer's registry digest. A second update
offers a nonexistent tag and checks the actual refusal while the application
stays ready. Only this run's registry and image tag are removed; the source
release's image is kept. Use ORCH_SMOKE_PORT to give concurrent runs their own
four ports. The default range is 41240–41243.

Evidence survives stack cleanup under `.tmp/deploy-smoke/evidence/<run>/distribution`:
candidate commit, exact offered manifest, pinned registry image, updater/registry
logs, check/failure counts and explicit unperformed cases. This is a local site
and registry demonstration. Published-site and macOS/Windows install/update/run
evidence remains unperformed until those actual platforms execute it; compilation
or the fixture tests does not establish that proof.

The manifest-copy behavior test is
`python3 -I -B -m unittest discover -s scripts/smoke/release-distribution -p 'test_*.py'`.
