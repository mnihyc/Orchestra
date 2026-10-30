# First binding on two machines

This scenario starts a fresh database and API, enrolls a provider-only connector and a
connector that hosts a real Codex in `orch-dev-harness`, and registers its directory as
an environment. It waits for real descriptors before creating any thread and checks
that the server offers and chooses `key_separated`, then completes the first turn
through the scripted model endpoint.
The thread's binding and fork preview agree on the harness and credential locations,
and an explicit unavailable provider-side placement is refused.

Build debug programs from the same checkout against the container's C runtime and
place the five executables in one directory, then run (the API needs OpenSSL;
the other programs may be static):

```sh
timeout 900 python3 scripts/smoke/kfix-binding/run.py --bin-dir <static-debug-directory> --scratch <unused-directory-on-the-larger-disk>
```

The scenario uses ports 41360–41368 and containers beginning `orchbind-kfix`.
It removes only those containers and waits for its own processes at exit. Logs and
the response evidence remain in the scratch directory; no descriptors or thread
records are inserted manually.

Add `--browser` to serve the built frontend at `orchbind-kfix.localhost:41366` and create the first thread
and its first input through that application. Sign in as `binding-owner` with the
throwaway password `binding scenario password`; the scenario records the topic and
resource identifiers in `browser.json` and allows fifteen minutes for this interaction.
After the first turn completes, inspect its rendered reply and create `browser.checked`
in the scratch directory to continue the policy checks and cleanup.
Use `--frontend-dir <built-frontend-directory>` to verify a separately built browser
checkout against this server. The browser directory is mounted read-only.
