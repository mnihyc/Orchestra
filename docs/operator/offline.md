# Running without internet access

Orch can use images, agent programs and models carried into an internal network. Nothing needs a public model service. Keep the release files and the digests you verified on the connected side together when carrying them in.

## Network inventory

| Operation | Source | Internal alternative |
| --- | --- | --- |
| Install or update Orch | A release site selected by the operator | A release directory passed to `deploy/update.sh --from` |
| Start the Compose services | The pinned Debian, PostgreSQL and nginx images | Load saved images on the Docker daemon before starting |
| Create or update an environment or provider runtime | Its template's image registry | Preloaded images, an internal registry, or a template's image archive |
| Build a template | Dockerfile base images, package repositories and optional Git context | Internal image names, an uploaded context and build arguments naming internal repositories and proxies |
| Run an agent program | The selected build's download address, on first use | The host's tool cache, an internal file mirror or an administrator-added build |
| Discover newer agent programs | Vendor channels, when explicitly requested or enabled | Carry in vendor checksums; a mirror supplies executable bytes, not checksum authority |
| Check for an Orch release | The configured release site, when explicitly requested or enabled | An internal release site; scheduled checks ship off |
| Refresh model prices | The configured rate source, when explicitly requested or enabled | The release's local rate catalogue or an internal source |
| Fetch a skill or template source | The URL or Git repository its manager imports | Upload an archive, or use an internal URL or repository |
| Run a model, summary, review or embedding request | The provider endpoint its manager configured | A provider served inside the network |
| Read subscription or known-key usage | That provider's vendor, after that provider is configured | Use an internal API provider; vendor subscriptions require their vendor |
| Sign in through an identity provider | Its configured discovery, authorization and token endpoints | An internal identity provider or local Orch sign-in |
| Send mail, webhooks, Telegram or browser push | The destinations people configured | Internal mail and webhooks; Inbox needs no external service |
| Issue or renew an optional edge certificate | The ACME service the operator selected | An existing TLS edge and certificates issued by the internal authority |
| Render the web application | Orch's own static assets and API | Fonts and scripts are served with the application |
| Use an agent's shell, browser or MCP server | Addresses requested by the person, agent or configured server | Internal services and package mirrors; enforce network policy outside Orch as needed |

Installing Docker and the host's command-line prerequisites is the operator's responsibility. A disconnected host needs those installed before it receives Orch. Builds and agent commands use the Docker daemon's network, which can differ from the network of the Compose services.

## Gather on a connected machine

Use Linux and the architecture of the internal hosts. Install Docker with Compose, a maintained Python 3.11 or newer with archive data filters, Bash, GNU coreutils and the ordinary installer prerequisites. First install refuses an older archive runtime before loading images or extracting anything. Obtain an ordinary Orch release through your usual trusted release process. The transfer checksum detects damage during copying; it does not authenticate an unfamiliar release publisher.

From a checkout of the release, run:

```bash
bash scripts/offline/gather.sh --release-dir ./release --out ./carry-in
```

Alternatively, `--release-url https://releases.example.org/orch/download/v1.0.0` reads `version.json` and its ordinary assets from that directory. Gathering verifies the release's sizes and BLAKE3 hashes using `b3sum`, or the release's own `orchctl`, as the ordinary updater does. The release remains the trust root for that checksum tool. The scripts do not build Orch, construct a new release or publish anything.

Gather saves the release's pinned images, the stock provider base and the Linux agent builds for `--arch x86_64` or `--arch aarch64` (the connected host's architecture by default). Its Docker daemon must hold the selected platform. For each agent build it reads checksum metadata directly from the vendor over HTTPS, compares that checksum with the release selection and checks the downloaded bytes. A mirror never supplies the expected vendor checksum. The vendor metadata and the exact checksum list travel with the directory.

The checkout supplies each program's declared vendor channel and platform names. This includes DeepSeek Harness wheels, whose checksums come from the index's [release-specific JSON API](https://docs.pypi.org/api/json/#get-a-release). A withdrawn wheel, an unsupported checksum scheme or a mismatch stops gathering before that program is downloaded. Vendor metadata stays within the JSON size limit, and only one reply is kept in memory. A checksum list that exceeds the import request limit must be gathered in smaller selections with `--harness-builds`.

Extra template images can be included with repeated `--image registry.example.org/team/base:version@sha256:<digest>`. Include a finished Local embeddings image only when needed:

```bash
bash scripts/offline/gather.sh --release-dir ./release --out ./carry-in \
  --local-embeddings registry.example.org/team/embeddings:version@sha256:<digest>
```

An embeddings image must already contain its model and runtime. Also include any extra base image needed by your templates. Gathering a normal image never builds its Dockerfile or installs packages. Keep the printed SHA-256 of `transfer.json` separately, then copy the entire directory into the internal network. Nothing from that directory belongs in the source repository. `ORCH_OFFLINE_COMMAND_TIMEOUT` changes the default 1,800-second image save/load/installer deadline; individual HTTP attempts give up after 30 seconds of silence, while active file transfers can continue.

## Install on the internal host

Prepare the normal `orch.toml` and private `.env` for your internal host, using the examples in the carried release's deployment archive. Set `system.public_url` and the gateway endpoints to addresses your internal clients and managed runtimes can reach. Choose a strong database password and an unused Compose project name in `.env`. Configure your existing TLS edge with certificates from the internal authority. Leave public ACME and unused external integrations off.

Check the files before making changes:

```bash
sha256sum ./carry-in/transfer.json
bash ./carry-in/install.sh --from ./carry-in --mode verify --sha256 <separately-carried-digest>
```

Compare the first command's output with the digest carried separately. Use installer scripts from a trusted checkout, or verify the carried `install.sh` and `transfer.py` against their entries in that independently checked manifest before executing the carried copy.

Install into an empty directory:

```bash
bash ./carry-in/install.sh --from ./carry-in --target ./orch \
  --config ./orch.toml --env ./.env --sha256 <separately-carried-digest>
```

The script checks all carried files before loading an image or starting the installer. It checks loaded image config IDs, sets Compose to use those local IDs with pulls disabled, and records the local image mapping in the node's ordinary configuration. Docker save/load may drop registry digest aliases; the mapping preserves the verified content rather than making a registry request to restore an alias. An absent mapped image fails with instructions to load it.

On engines that use manifest IDs instead of config IDs, the verifier temporarily re-exports each loaded image to check its exact config bytes. Set `TMPDIR` to a filesystem with room for your largest image if the host's temporary directory is small.

The ordinary `deploy/update.sh first-start --from` then verifies payload hashes, creates the system's own certificates, applies migrations and checks readiness. Its normal backup, rollback and recovery behavior remains in force. Finish the normal first-administrator setup in the browser. The bundled node enrolls through the internal gateway; additional nodes and connected machines download their installers and binaries from this Orch system's own Downloads page.

The generated certificate files are public and may be mode `0644`; other accounts must not be able to
change or execute them. Private keys, tokens, unknown files in the secrets directory and `.env` stay
owner-only (`0600`). `orchctl doctor` checks these same declarations and never prints credential contents.

On another Docker host, run `install.sh --from ./carry-in --mode load` before starting work there. Give that node the same `offline_images` mappings from the first host's generated config, or use an internal registry or archive-backed templates. A loaded image on one daemon does not populate another daemon's cache.

A restricted environment also needs the node's trusted proxy image. Choose the local immutable image ID
of a verified carried image with `/bin/sh` and the usual core utilities; the carried Debian base works.
Before the node's first start, set `[node.docker].restricted_proxy_image` to that ID. After the first
start, driver settings live in the machine's single policy file: use `orch-node policy set drivers` to
keep its existing driver choices with `configurations.docker.restricted_proxy_image` updated, then restart
the node. Editing the earlier `orch.toml` does not replace that imported policy. The helper is chosen
explicitly by the operator; a runtime's image is never automatically trusted as a host proxy.

When that host can reach only the direct gateway door, carry the node executable too. After verifying
the directory, prepare its ordinary node configuration with that gateway address and its local image
mappings. As root, place the executable and enroll with the host token from Add host (or the bound token
from Enroll again for a replacement). These commands use the carried Linux x86-64 build; choose the
matching aarch64 filename on that architecture:

```sh
install -m 755 ./carry-in/release/orch-node-linux-x86_64-musl /usr/local/bin/orch-node
/usr/local/bin/orch-node --config /etc/orch/orch.toml enroll --url wss://gateway.internal.example:9443 --token '<host token>'
/usr/local/bin/orch-node --config /etc/orch/orch.toml run
```

The token pins Orch's authority at first contact, so this path needs no browser-facing route or
extra certificate in the node's device store. Its environments use the node's configured addresses.
Restore the node's locally saved policy when recovering a lost data directory; a token does not silently
hand that policy over. The installer commands shown in the browser still download from the people's
address, which must be reachable while using those commands. See [the edge guide](edge.md) for both doors.

## Agent builds and internal mirrors

Serve `carry-in/mirror/` on an internal static HTTPS server. The layout is `<mirror>/<vendor-host>/<vendor-path>`, preserving the normal download path. Give this address to the system setting `agents.harness_mirror`. Install your internal CA on the hosts and services that fetch from it. Do not disable TLS verification.

For the stack's outgoing HTTPS, put each internal authority's public certificate in a separate PEM file under `deploy/config/ca-certificates/`, make it readable by the services, and run `openssl rehash ./config/ca-certificates` from the deployment directory. Restart API, gateway, worker and node. They keep the bundled public roots and add this read-only directory; no Compose edit is needed. Standalone processes use their machine's store. Managed provider templates and Docker daemons need the trust of the authorities their own endpoints use. The directory follows the [native certificate loader's hashed-directory format](https://docs.rs/crate/rustls-native-certs/0.8.1/source/src/lib.rs).

After signing in as system administrator, enable As admin for that session. Put its browser-session bearer token from the normal authentication flow in a private file and import the carried vendor checksum list. A narrowed API token cannot replace that browser session:

```bash
bash ./carry-in/install.sh --from ./carry-in --mode import \
  --api-url https://orch.internal.example/api/v1 --token-file ./admin-session
```

The command prints the exact list's SHA-256, verifies the server's returned digest and records one administrator audit entry for the whole import. Tokens are read from the file, never placed in command arguments or printed. The request follows no redirects. Remove the temporary session file using your usual secret-handling procedure after use. The tool cache accepts a mirrored executable only when its bytes match a release checksum or a checksum the administrator explicitly imported or added.

An ordinary mirror supplies executable bytes. Refreshing channels while a mirror is configured asks for an imported vendor list instead of trusting mirrored metadata. A connected administrator can gather a new exact vendor build with `--harness-builds ./builds.json`, using the same build-row format as `deploy/harness-builds.json`, then import its list internally. A new build reaches a running environment through its normal Update operation.

Only if you intentionally delegate executable publishing to the mirror, enable `agents.harness_trust_mirror`. It ships off. Builds discovered under that choice say **Checksum from mirror (not the vendor's)** in Admin › Harnesses. Turning it off removes their eligibility for automatic checks and selection. A compatibility check does not establish publisher trust.

## Templates, models and integrations

Use internal registry image names in templates, or their ordinary image-archive source. For a Dockerfile, supply proxy and repository addresses through `[node.docker].build_args`; arguments explicitly set on the template take precedence. The Dockerfile must use the repository arguments you give it. For example:

```toml
[node.docker.build_args]
HTTP_PROXY = "http://proxy.internal.example:3128"
HTTPS_PROXY = "http://proxy.internal.example:3128"
NO_PROXY = "localhost,.internal.example"
APT_MIRROR = "https://packages.internal.example/debian"
```

These arguments are stored in clear; use no credentials in them. Use the ordinary protected registry credential files for authenticated registries. The daemon's own registry mirrors and certificate trust are configured on that daemon. Package-manager mirrors are specific to the Dockerfile, workspace or image. An uploaded build context avoids fetching a Git repository.

Create an API provider pointing to a model server inside the network, with the protocols and models it actually serves. Agent turns, summaries, descriptions and embeddings use that same provider entrance. Choose its local model for the relevant place's automation or semantic-search settings; lexical search works without embeddings. For Local embeddings, select the carried finished image through the ordinary editable provider template so provisioning downloads neither llama.cpp nor a model. If rebuilding it, override the template's download build arguments with internal mirror URLs while retaining its pinned checksums.

An internal model served over HTTP requires the place's explicit `providers.allow_http_upstreams` setting and the provider machine's existing upstream policy to allow that address. Internal HTTPS endpoints use the hosts' ordinary certificate trust.

Local accounts and an internal OpenID Connect issuer both work. Inbox needs no external delivery service; internal SMTP and webhooks can deliver outside the browser within the network. Vendor subscriptions, Telegram and browser push retain their vendor requirements. Keep them unconfigured where those services cannot be reached. Rate catalogues ship locally; optional refreshes and release checks can point to internal sources. Image-tag checks ship off (`node.image_check_hours = 0`); pin templates or opt into checks against your internal registry.

## Update

Gather the next ordinary release on the connected side into a new empty directory and carry it in. Back up the installation as usual, then run:

```bash
bash ./next-carry-in/install.sh --from ./next-carry-in --mode update \
  --target ./orch/deploy --sha256 <separately-carried-digest> -- --yes
```

The script verifies and loads the new images, retains the operator's configuration and secrets, refreshes only the requested local image mapping and Compose image override, and invokes the ordinary updater with `--from`. It never falls back to downloading a missing carried file. Existing runtimes keep their current images and agent builds until their ordinary Update operation is requested. Import the new agent checksum list as a separate explicit administrator act when it changes.

For an interrupted transaction use the ordinary `deploy/update.sh recover`; for a failed update use its normal rollback procedure. Keep the old gathered directory and backups for that recovery. Inspect `docker compose config` and the node's configured image mappings when adding a new service or template image: everything that service needs must exist on its own Docker daemon or in an explicitly selected internal source.

See [Ways in and Certificates](edge.md) for company authorities, Orch’s optional HTTPS, machine address lists and Enroll again. Orch requires no ACME or external DNS service.

Build records left by an earlier version without a checksum origin are not executed or offered. Import their vendor checksum list, or explicitly add their expected checksum again as an administrator, before using them.
