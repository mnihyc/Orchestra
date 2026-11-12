# Registered native notification cases

Native notification byte scripts and their complete version manifests now live in `tests/fixtures/harness/codex/<version>` at the workspace root. upgrade_notifications loads them through the common Corpus and uses the existing shared replay_io helper, production RPC reader and public Mapper. It does not define another replay format or parser.

The corpus README explains exact source provenance, legacy dialect versus accepted startup policy, explicit unsupported decisions, and the actual actor seam for controls. Every notification expectation keeps ordered event counts and exact arrays with no ignored marker or error.
