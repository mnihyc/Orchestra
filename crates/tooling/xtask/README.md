# orch-xtask

Development checks, contract generation and release packaging.

OpenAPI rendering belongs to `orch-api-contract`; both this tool and the API server supply the
realtime schema registrar and the agent contract's (the descriptor of an agent program, which the API
contract is below) to it. Contract generation does not depend on the API implementation.

`cargo xtask names` fails when shared code (everything in `crates/` and `apps/` but the tests, the composition roots
and the data that names programs, which the check lists with the reason for each) says the name of an agent program
or of a vendor of one in an identifier or a string. `cargo xtask verify` runs it first.

`cargo xtask verify` runs the formatting, Clippy, compilation, documentation and
dependency checks. `cargo xtask contracts` writes the OpenAPI document and the
browser's types, settings help registry and provider implementer schemas/tables/
constants/worked examples from the contract crates, and `--check` fails when they differ
from the committed ones. `cargo xtask package --out DIR` builds the files of a
release for the architecture Docker runs on and the manifest that lists them
with their sizes and BLAKE3 hashes; `--check` validates such a directory again
(the manifest, every hash, the harness builds it pins, what each archive holds, and that the shipped
`orch.toml` is accepted by the programs), and `--manifest-only` writes the
manifest of the files a directory holds, for a release put together from
several machines. `cargo xtask image [--version V] [--fresh]` builds the default
environment image alone, as `orch-env-base:V`, and refuses an image whose labels do
not say the versions its recipe pins (`package` builds it the same way, and
`--fresh-image` there, like `--fresh` here, builds without Docker's layer cache). `package` also builds the developer image `orch-env-dev:V` on it from `images/orch-env-dev` and holds it to the versions that recipe pins, so that the release names both.
`cargo xtask identity frontend` answers the browser
application's build with the identity a Rust build gets from its build script.
The server programs are compiled in a container of the distribution the Compose
services run, and the connector, node and `orchctl` as static musl programs in
the same container; nothing is installed on the machine that runs the command.
The release modules are tested on archives and manifests made in a temporary
directory; packaging itself is run by the deployment smoke scenarios.
`cargo xtask verify --offline -p PACKAGE` checks one package with cached inputs;
its dependency gate still checks the complete workspace graph. Repeated `-p`
options check each named package in turn. Release archives carry one generated
`THIRD_PARTY_NOTICES` file with package terms or standard SPDX fallback texts,
including the separately licensed MPL files used for push encryption.

Stock runtime images and the default environment base are selected by index digest and recorded in
the manifest. The [stock image script](../../../scripts/release/README.md) reviews or explicitly
refreshes those pins, and CI checks them offline beside the pinned Actions.

`package --native-connector TARGET --out DIR` builds, unit-tests and runs a
connector on that target's actual operating system and processor, recording its
size, BLAKE3 and SHA-256 in an input manifest. Repeated `package --connectors DIR`
imports only verified builds of the same commit, release and link protocol. They
are release assets and are also in every server archive's `connectors/` directory
with their platform-specific download URLs. `--require-all-connectors` refuses
missing declared targets; the release workflow uses it for both packaging and
checking. Local smoke packaging without it continues to build the host platform.

| Module | Responsibility |
|--------|----------------|
| [release/identity](src/release/identity.rs) | Build identity for a component Cargo does not build: the frontend's, asked for by its build. |
| [contracts/providers](src/contracts/providers.rs) | Provider implementer documentation derives its wire shapes and limits from the contracts. |
| [release/manifest](src/release/manifest.rs) | Release metadata and asset hashes. |
| [release/package](src/release/package.rs) | Server, frontend, connector, node, CLI and deployment payloads. |
| [release/image](src/release/image.rs) | The default environment image: built from its recipe and held to what the recipe pins. |
| [release/archive](src/release/archive.rs) | Archives written the same way every time, and read back to see what is in them. |
| [release/container](src/release/container.rs) | Compiling and reading inside the builder image. |
| [release/connectors](src/release/connectors.rs) | Native connector builds verified before inclusion in server downloads. |
| [release/stock](src/release/stock.rs) | Immutable stock image selections written into the release manifest. |
| [release/licences](src/release/licences.rs) | The licences of what is compiled into a release's programs. |
| [release/font_names](src/release/font_names.rs) | Copyright and licence strings from an OpenType font's name table. |

See [Contributing](../../../CONTRIBUTING.md) for the implemented commands
and build-counter behavior.
