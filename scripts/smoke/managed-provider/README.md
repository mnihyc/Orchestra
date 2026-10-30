# managed-provider scenario

A subscription of each of the two built-in harnesses, Codex and Claude Code, that Orch looks after: a runtime of its own on the
bundled host, signed in from the API, stopped and started, signed out and deleted. The system is started by the updater from a
packaged release. Without the key of a model service no model is asked, and no real credential is given.

1. **The shipped provider runtime.** The fresh database supplies the Provider runtime template, which builds the small,
   pinned [provider image](../../../images/orch-provider-base/README.md) with the C/C++ libraries both harnesses need.
   No template is created, edited or selected by an override. The scenario checks the runtime uses this template, its
   original revision remains unchanged, its root is read-only, and it has no shell or extra privileges.
2. **The job that makes the runtime.** `POST /providers` with a profile whose adapter has a sign-in of its own makes a provider
   and the job `provider_provision`: its phases run in order, the unit of the system is running, named by the runtime, its `state`
   volume is kept under the provider's name, the harness build is mounted in it read-only, and the connector in it plays the roles
   of a provider's runtime and of a harness's place.
3. **Signing in.** The provider is signed in by the method that imports a credential, with a credential that is not real in the form
   the harness takes (an `auth.json` for Codex, a token for Claude Code): the harness takes it, nothing of it waits sealed in the
   database any longer, and it is in one file of the unit's own volume that only its account can read. Then it is looked for where it
   must not be (the database, the logs of the services, the environment and command line of the unit's processes, what the daemon
   says of the unit), after a control that shows the search finds it where it is. Since the provider has no inspection tools,
   a separate container of the services' distribution reads its state volume and process namespace as the same unprivileged user,
   with a read-only root and state mount, no network and no capabilities. A failed inspection fails the custody check.
4. **Stopped, started, signed out, deleted.** A runtime stopped by hand keeps its unit and its volume and leaves the provider with
   no machine; started again it is the same unit and the sign-in is in it; signed out, nothing of the credential is left; deleted,
   the unit and its volume are gone.

5. **A real model, once each** (only with `ORCH_PROVIDER_REAL_ENV=<file with OPENROUTER_API_KEY>`): an API provider made through the API keeps its key
   in the unchanged shipped runtime, and each harness runs there with the `separated` arrangement and makes one small request.
   The scenario verifies its actual harness machine and the ledger's tokens, credits and forwarder origin. The key is looked
   for where it must not be (the environment's unit, the processes of both units, what the daemon says of them, the logs of the services and the
   database) by a search that hands it to grep as a file descriptor and prints nothing, after a control that shows the search finds a key where there is one.

```sh
ORCH_SMOKE_RELEASE_A=/path/to/a/release scripts/smoke/managed-provider/run.sh
# Include the model and ledger checks with an authorized test service key:
ORCH_SMOKE_RELEASE_A=/path/to/a/release ORCH_PROVIDER_REAL_ENV=/path/to/model-service.env scripts/smoke/managed-provider/run.sh
```

The release has to be one whose manifest lists the harness builds (any release packaged from this tree does), and the host of the
node needs access to the internet: the node downloads the builds from the vendors' addresses, about 420 MB the first time. Besides
what the other scenarios need it needs `jq` (and `node` for the real model). Ports 41350 (the web edge, on the loopback) and 41351 (the pinned listener, on the
Docker bridge's address), which `ORCH_SMOKE_PORT` moves. At the end it takes down the stack, removes the units the node made for it
and their volumes, and the directory it worked in. The driver's shared network is left for other runtimes.

The optional subscription proofs are separate from the synthetic material and API-key model checks above. `subscription.sh` uses fresh providers made through Orch and the unchanged managed runtime. Nothing is read from a machine's Codex or Claude home.

For an authorized interactive proof, set `ORCH_SMOKE_NATIVE_SIGNIN=1`. Each harness's reported interactive method is selected through `/providers/setup`. The scenario cancels one flow, then waits for a human to reject or let another expire, and finally waits for the dedicated test account to authorize a fresh flow. It displays only the address/code/fields the product displays. Complete any requested fields on the provider's Sign in page. Current authenticated facts, explicit stop/start and an API restart must all succeed. The default wait for each human outcome is 1,800 seconds; `ORCH_SMOKE_SIGNIN_WAIT_SECS` can raise it to 7,200. A timeout fails the proof.

For imported real-subscription turns, supply two owner-readable JSON files (mode 0600 or 0400), each an exact login body with its declared import method and string `values`. For example, a Codex auth-file import has `method: "auth_file"` and an `auth_json` value containing the exported file's text; a Claude setup-token import uses the field names its method declares. Keep one renewal owner for a whole native sign-in. The script reads only the explicitly supplied files, sends them as HTTP bodies by file name, and withholds their bodies on a refusal.

```sh
ORCH_SMOKE_RELEASE_A=./release ORCH_SMOKE_NATIVE_SIGNIN=1 scripts/smoke/managed-provider/run.sh
# Explicit model choices authorize the two small real turns, one before and one after restart, for each harness:
ORCH_SMOKE_RELEASE_A=./release ORCH_SMOKE_SUBSCRIPTION_IMPORT=1 \
  ORCH_SMOKE_CODEX_LOGIN_JSON=./private/codex-login.json ORCH_SMOKE_CLAUDE_LOGIN_JSON=./private/claude-login.json \
  ORCH_SMOKE_CODEX_MODEL=your-codex-routing-name ORCH_SMOKE_CLAUDE_MODEL=your-claude-routing-name \
  scripts/smoke/managed-provider/run.sh
```

Choose one gate per run. Native mode makes no model request unless model names are explicitly supplied; import mode requires both names. Each real turn must call a tool and answer, then the same thread must produce a later counted ledger turn after the API and provider runtime restart. The proof keeps the same unit and runtime row. Only safe ids, turn numbers and restart facts enter `subscription-*-proof.json`; the dedicated test providers are signed out and removed. These files and transcripts live in the scenario's private working directory; `ORCH_SMOKE_KEEP=1` retains it for review. Merely writing or skipping a gate proves no vendor sign-in or device delivery.
