# Run Orch without internet access

Install, update and use Orch on hosts that cannot reach the internet, by gathering everything on a connected machine and carrying it in. Nothing needs a public model service. At the end you have a running system whose images, agent programs and models all come from inside your network.

## Before you start

You need:

- **A connected Linux machine** of the architecture of your internal hosts, with Docker and Compose, Bash, GNU coreutils, and Python 3.11 or newer with archive data filters. The first install refuses an older Python before it loads an image or unpacks anything.
- **An Orch release** from your usual trusted release process, as a directory (`--release-dir`) or a release address. The checksum of the transfer detects damage while you copy; it does not prove who published an unfamiliar release.
- **Docker and the host prerequisites on each internal host**, installed beforehand. They are your job: [Install Orch](deployment.md) lists them. A disconnected host cannot fetch them later.
- **A way to carry files in**, and a second way to carry one short checksum. Keep them apart: the checksum is how you notice a change.
- **Internal places to read from** where you need them: an internal registry or image archives for templates, and an internal model server.

## 1. Gather on the connected machine

1. From a checkout of the release, run:

   ```bash
   bash scripts/offline/gather.sh --release-dir ./release --out ./carry-in
   ```

   To read a release from an address instead, use `--release-url https://releases.example.org/orch/download/v1.0.0`. The script reads `version.json` and its ordinary files from that directory. The scripts never build Orch, make a release or publish anything.

2. Add what your templates need:
   - `--image registry.example.org/team/base:version@sha256:<digest>`, repeated for each extra template image.
   - `--local-embeddings registry.example.org/team/embeddings:version@sha256:<digest>` for a finished Local embeddings image. The image must already contain its model and runtime. Gathering never builds a Dockerfile or installs packages.
   - `--arch x86_64` or `--arch aarch64` if the internal hosts differ from this machine. Its Docker daemon must hold the platform you choose.
   - `--harness-builds ./builds.json` for an exact new agent program build, in the row format of `deploy/harness-builds.json`. Use it too when a checksum list is larger than one import request allows: gather it in smaller selections.

3. Write down the SHA-256 of `transfer.json` that the script prints, and keep it separately from the directory. Then copy the whole directory into the internal network. Nothing in it belongs in a source repository.

**What gathering does.** It checks the release's file sizes and BLAKE3 hashes with `b3sum` or the release's own `orchctl`, as the ordinary updater does; the release stays the root of trust for that tool. It saves the release's pinned images, the stock provider base and the Linux agent program builds. For each agent program it reads the checksum from the vendor over HTTPS, compares it with the release's choice and checks the downloaded bytes. DeepSeek Harness wheels use the index's [JSON API for a release](https://docs.pypi.org/api/json/#get-a-release). A mirror never supplies the expected checksum. A withdrawn wheel, a checksum scheme Orch does not support, or a mismatch stops the gathering before that program is downloaded.

Two limits apply while gathering. `ORCH_OFFLINE_COMMAND_TIMEOUT` changes the 1,800-second default for saving or loading an image; one HTTP attempt gives up after 30 seconds of silence, while a transfer that is moving continues.

**What you will see.** The script lists each item as it saves it, and ends with `Gathered <version>; transfer manifest SHA-256: <digest>`.

## 2. Check the carried files

On the internal host:

```bash
sha256sum ./carry-in/transfer.json
bash ./carry-in/install.sh --from ./carry-in --mode verify --sha256 <separately-carried-digest>
```

The first command prints a digest: compare it with the one you carried separately. The second prints `Verified transfer manifest SHA-256: <digest>` when every carried file matches.

Run the installer scripts from a trusted checkout. If you run the carried `install.sh` and `transfer.py`, first check them against their entries in the manifest you verified.

## 3. Install

1. Prepare `orch.toml` and a private `.env` for the host, from the examples in the carried release. Set `system.public_url` and the gateway endpoints to addresses that your internal clients and managed environments can reach. Choose a strong database password and an unused Compose project name. Leave public ACME and the integrations you do not use switched off.
2. Put your existing HTTPS front in place with certificates from your internal authority ([Run the HTTPS front](edge.md)).
3. Install into an empty directory:

   ```bash
   bash ./carry-in/install.sh --from ./carry-in --target ./orch \
     --config ./orch.toml --env ./.env --sha256 <separately-carried-digest>
   ```

4. Finish the first-administrator setup in the browser. The bundled node enrolls through the internal gateway. Other nodes and machines download their installers from this system's own **Downloads** page.

**What the installer does.** It checks every carried file before it loads an image or starts the updater. It then checks the config IDs of the loaded images, tells Compose to use those local images with pulls turned off, and records the mapping in the node's configuration. Docker can drop a registry digest alias when it saves and loads an image, so the mapping keeps the verified content and makes no registry request. A mapped image that is missing stops the install with instructions to load it. On an engine that uses manifest IDs, the check briefly re-exports each image; set `TMPDIR` to a filesystem with room for your largest image if the default is small.

Then `deploy/update.sh first-start --from` verifies the payload hashes, creates the system's certificates, applies the migrations and checks readiness, with its usual backup, rollback and recovery.

**Check that it works.** Follow [Check that it works](deployment.md#5-check-that-it-works) in the install guide. `orchctl doctor` also checks the file permissions: generated certificate files are public and may be mode `0644`, but private keys, tokens, unknown files in the secrets directory and `.env` must be owner-only (`0600`). It never prints a credential.

A restricted environment also needs the node's trusted proxy image. Choose the local immutable image ID
of a verified carried image with `/bin/sh` and the usual core utilities; the carried Debian base works.
Use `orch-node policy set drivers` to retain the existing driver choices with
`configurations.docker.restricted_proxy_image` set to that image ID, then restart the node.
Driver policy stays in the machine's single `config.toml`; the deployment file is not another policy
source. The helper is chosen explicitly by the operator; a runtime's image is never automatically
trusted as a host proxy.

## Add another host

1. On the other Docker host, run `install.sh --from ./carry-in --mode load`. An image loaded on one daemon does not fill the cache of another.
2. Give that node the same `offline_images` mappings from the first host's generated configuration, or use an internal registry or templates backed by image archives.
3. For restricted environments, choose a proxy image for the node. Use the local immutable image ID of a verified carried image that has `/bin/sh` and the usual core utilities; the carried Debian base works. Before the node's first start, set `[node.docker].restricted_proxy_image` to that ID. After the first start the driver settings live in the machine's policy file, so run `orch-node policy set drivers` with `configurations.docker.restricted_proxy_image` updated, then restart the node. Editing `orch.toml` later does not replace the imported policy. Orch never trusts an environment's own image as a host proxy.
4. If the host can reach only the direct gateway way in, carry the node program too. Verify the directory, prepare the node's configuration with the gateway address and the image mappings, and run as root. Use the matching `aarch64` filename on that architecture:

   ```sh
   install -m 755 ./carry-in/release/orch-node-linux-x86_64-musl /usr/local/bin/orch-node
   /usr/local/bin/orch-node --config /etc/orch/orch.toml enroll --url wss://gateway.internal.example:9443 --token '<host token>'
   /usr/local/bin/orch-node --config /etc/orch/orch.toml run
   ```

   Use the host token from **Add host**, or the token from **Enroll again** for a replacement. The token pins Orch's authority at the first contact, so the node needs no browser-facing route and no extra certificate in its store. Restore the node's saved policy when you recover a lost data directory: a token never hands that policy over. The installer commands the browser shows still download from the address people use, which must be reachable while you run them. [Run the HTTPS front](edge.md) explains both doors.

## Serve agent programs from an internal mirror

1. Serve `carry-in/mirror/` from an internal static HTTPS server, in the layout `<mirror>/<vendor-host>/<vendor-path>`. Install your internal authority on every host and service that fetches from it, and keep certificate checking on.
2. Set the system setting `agents.harness_mirror` to the mirror's address.
3. Make the stack trust your authorities for its own outgoing HTTPS. Put each public certificate in its own PEM file under `deploy/config/ca-certificates/`, make it readable by the services, and run `openssl rehash ./config/ca-certificates` from the deployment directory. Restart the API, gateway, worker and node. They keep the bundled public roots and add this directory; no Compose edit is needed. The directory uses the [hashed-directory format of the native certificate loader](https://docs.rs/crate/rustls-native-certs/0.8.1/source/src/lib.rs). Programs that run on their own use their machine's store, and managed provider templates and Docker daemons need the authorities of the endpoints they call.
4. Import the vendor checksum list. Use a current system administrator's browser session in **As admin** mode, or an account key allowed to perform `AdminHarnessChecksumsImport` with **Manage** on the system. Put the credential in a private file and use the API root that matches it:

   ```bash
   # Browser session access token (oat_):
   bash ./carry-in/install.sh --from ./carry-in --mode import \
     --api-url https://orch.internal.example/api/v1 --token-file ./admin-session

   # Account key (opat_):
   bash ./carry-in/install.sh --from ./carry-in --mode import \
     --api-url https://orch.internal.example/openapi/v1 --token-file ./account-key
   ```

   It prints `Imported N vendor checksums; the administrator act is audited` and records one audit entry for the whole import. The token is read from the file and never printed or put in an argument. The request follows no redirects. Remove the file afterwards.

A mirror supplies executable bytes, never checksum authority. The tool cache accepts a mirrored executable only when its bytes match a release checksum or a checksum an administrator imported or added. While a mirror is set, refreshing the channels asks for an imported vendor list instead of trusting mirrored metadata. To bring a new exact build in, gather it with `--harness-builds`, import its list, and update a running environment with its normal **Update**.

Only if you deliberately leave executable publishing to the mirror, turn on `agents.harness_trust_mirror` (off by default). Builds found that way read **Checksum from mirror (not the vendor's)** in **Management → Agent programs**. Turning it off makes them ineligible for automatic checks and selection. A compatibility check does not establish who published a build.

A build record from an earlier version that has no checksum origin is neither run nor offered. Import its vendor checksum list, or add its expected checksum again as an administrator.

## Use internal registries, models and services

- **Templates.** Use internal image names, or a template's image archive. For a Dockerfile, give proxy and repository addresses in `[node.docker].build_args`; arguments set on the template win. The Dockerfile must use the arguments you give it. These values are stored in clear text, so put no credentials in them. Use the protected registry credential files for registries that need a sign-in. Registry mirrors and certificate trust of the daemon are set on that daemon. An uploaded build context avoids fetching a Git repository.

  ```toml
  [node.docker.build_args]
  HTTP_PROXY = "http://proxy.internal.example:3128"
  HTTPS_PROXY = "http://proxy.internal.example:3128"
  NO_PROXY = "localhost,.internal.example"
  APT_MIRROR = "https://packages.internal.example/debian"
  ```

- **Models.** Add an API provider that points at a model server inside the network, with the protocols and models it really serves. Agent turns, summaries, descriptions and embeddings all use that entrance. Choose the model in the automation or semantic-search settings of the place; word search works without embeddings. For Local embeddings, choose the carried image in the editable provider template, so provisioning downloads neither llama.cpp nor a model. If you rebuild it, override the template's download build arguments with internal mirror addresses and keep its pinned checksums.
- **A model over plain HTTP.** Turn on the place's `providers.allow_http_upstreams`, and make sure the upstream policy of the provider's machine allows the address. Internal HTTPS endpoints use the host's certificate trust.
- **Sign-in.** Local accounts and an internal OpenID Connect issuer both work.
- **Notices.** The Inbox needs no outside service. Internal SMTP and webhooks can deliver outside the browser. Vendor subscriptions, Telegram and browser push need their vendors: leave them unconfigured where those cannot be reached.
- **Prices and releases.** The rate catalogue ships with the release. Optional refreshes and release checks can point at internal sources. Image-tag checks are off (`node.image_check_hours = 0`); pin your templates, or turn the checks on against your internal registry.

## What Orch contacts, and what to use instead

Nothing here is contacted unless someone set it up: the worker reaches a mail server, the Telegram Bot API, webhooks and browser push services only when configured; the API reaches an identity provider or a rate-catalogue address only when an administrator set one; the node reaches image registries and whatever a Dockerfile fetches; and `update.sh` reaches the release site only when you run it without a directory. Builds and agent commands use the Docker daemon's network, which can differ from the network of the Compose services.

| Operation | Source | Internal alternative |
| --- | --- | --- |
| Install or update Orch | A release site | A release directory passed to `deploy/update.sh --from` |
| Start the Compose services | The pinned Debian, PostgreSQL and nginx images | Saved images loaded on the Docker daemon before the start |
| Create or update an environment or managed provider | The template's image registry | Preloaded images, an internal registry, or a template's image archive |
| Build a template | Dockerfile base images, package repositories, an optional Git context | Internal image names, an uploaded context, build arguments naming internal repositories and proxies |
| Run an agent program | The selected build's download address, on first use | The host's tool cache, an internal file mirror, or a build an administrator added |
| Find newer agent programs | Vendor channels, when requested or turned on | Carried vendor checksums; a mirror supplies bytes, not checksum authority |
| Check for an Orch release | The configured release site, when requested or turned on | An internal release site; scheduled checks are off |
| Refresh model prices | The configured rate source, when requested or turned on | The release's local rate catalogue, or an internal source |
| Fetch a skill or template source | The URL or Git repository a manager imports | An uploaded archive, or an internal URL or repository |
| Run a model, summary, review or embedding request | The provider endpoint a manager configured | A provider served inside the network |
| Read subscription or key usage | That provider's vendor | An internal API provider; vendor subscriptions need their vendor |
| Sign in through an identity provider | Its discovery, authorization and token endpoints | An internal identity provider, or local sign-in |
| Send mail, webhooks, Telegram or browser push | The destinations people configured | Internal mail and webhooks; the Inbox needs no outside service |
| Issue or renew an edge certificate | The ACME service you chose | An existing HTTPS front with certificates from the internal authority |
| Show the web application | Orch's own files and API | Fonts and scripts are served with the application |
| Use an agent's terminal, browser or MCP server | Addresses the person, agent or server asks for | Internal services and package mirrors; enforce network policy outside Orch |

Without the carry-in installer, load the three stock images (`postgres:17-alpine`, `debian:bookworm-slim` and `nginx:1.30-alpine`) with `docker load` before the first start. The release's Compose file names each by its tag and image-index digest. A host that cannot reach the database image's build sources also needs the built database image, whose Compose name is in [the database recipe](../../deploy/postgres/README.md). The [release image script](../../scripts/release/README.md) prints the current indexes for review and changes the pins only when you pass `--write`.

## Update offline

1. On the connected side, gather the next release into a new empty directory and carry it in.
2. Back up the installation ([Back up and restore](backup-and-restore.md)).
3. Run:

   ```bash
   bash ./next-carry-in/install.sh --from ./next-carry-in --mode update \
     --target ./orch/deploy --sha256 <separately-carried-digest> -- --yes
   ```

The script verifies and loads the new images, keeps your configuration and secrets, refreshes only the requested local image mapping and Compose image override, and runs the ordinary updater with `--from`. It never falls back to downloading a file that is missing from the carried directory. Existing environments keep their images and agent program builds until you run **Update** on them. When the agent checksum list changes, import it again as its own administrator act.

## If it does not work

- **The install stops with a missing image.** Load it with `install.sh --from ./carry-in --mode load` on that host's Docker daemon. When you add a service or template image, inspect `docker compose config` and the node's image mappings: everything the service needs must exist on its own daemon or in an internal source you chose.
- **An update was interrupted.** Run `deploy/update.sh recover`. For a failed update use the normal rollback ([Update and roll back](upgrades.md)). Keep the old gathered directory and your backups for this.
- **Verification fails.** Do not install. Gather again, and compare the digest you carried separately with the one the check prints.
- **A mirrored agent program is refused.** Its bytes do not match a release checksum or an imported one. Import the vendor checksum list as above.

Company authorities, Orch's optional HTTPS, machine address lists and **Enroll again** are in [Run the HTTPS front](edge.md). Orch needs no ACME or outside DNS service.
