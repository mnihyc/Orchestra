# Provider runtime image

The Provider runtime template builds this recipe on its host. It contains the C/C++ runtime libraries the native
Claude Code build needs, certificates for HTTPS, and the `nonroot` account (uid and gid 65532). Codex's static
Linux build runs on the same image. The connector and both harnesses come from the node's tool cache, mounted
read-only; the image contains no shell, package manager, browser, display server or workspace tools.

The [Dockerfile](Dockerfile) pins the multi-platform index of `gcr.io/distroless/cc-debian13:nonroot`. It needs
no context, build arguments or unpublished Orch image. The seed embeds the recipe as an ordinary template
build source, and hosts cache the built image by its content. Hosts that create this default must allow builds,
as bundled hosts do by default. Operators can select another provider template or edit this one through Orch.
Starting a newer release preserves every existing template, including an operator's image, limits and Use row.

On x86_64 an export contains 27.7 MB of regular files (27,709,854 bytes), and Docker lists 42.9 MB of local
storage. Docker's image-inspection size is 10.7 MB (10,689,200 bytes), which does not describe the unpacked files.
Its runtime keeps the
seeded limits of two CPUs, 2 GiB and 512 processes, a read-only root, no new privileges and all capabilities
dropped. Only the provider's state volume persists; `/tmp` is temporary. Nothing of an environment is mounted
in it. The Apache-2.0 image label describes this recipe; the base's libraries carry their own licenses.

```sh
docker build --tag orch-provider-base:local images/orch-provider-base
```

[The managed-provider scenario](../../scripts/smoke/managed-provider/README.md) starts a packaged release on a
fresh database, uses the shipped template without an override, runs both actual harnesses there, checks
credential custody through a separate inspector, and stops, starts, signs out and deletes each provider.
Its model section runs both harnesses with the provider and checks the usage ledger. The shipped harnesses,
their credential lifecycle and one real model turn each are verified on x86_64 with Codex 0.162.1 and Claude Code
2.1.296. The pinned base also has an arm64 image, whose runtime is not verified here.
