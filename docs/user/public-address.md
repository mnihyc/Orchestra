---
group: operations
title: Public address
screens: /admin/$section, /login
keywords: public address, primary, alternate, origin, hostname, certificate, reset, revision
summary: Apply a prepared browser origin and recover its file seed from the local host.
---

# Public address

## Change address

In **Management → System**, enter the primary address and any alternate addresses, then choose **Save and open**. Orch prepares a new managed frontend certificate generation and waits for its workers to load it. An external edge must already serve this same installation. DNS and externally owned certificates remain with their operator.

The applied revision changes only after preparation succeeds. Failure leaves the previous addresses in force and your draft beside its reason. Other browsers keep their work and see one line with **Open**; they are not reloaded. An admitted sign-in, issued link or exported package keeps the address it began with.

The example-address state means the shipped primary has not yet been replaced. File `public_url` and `alternate_urls` seed the system store once; later file edits do not silently replace the committed snapshot.

## Local recovery

Run where the deployment's configuration and local database are available:

```sh
orchctl public-address reset --config ./config/orch.toml
```

Recovery of an already admitted file seed or current set needs no working HTTP hostname. A novel file origin follows the same preparation before its revision-checked reset. No credential is printed. `orchctl public-address get --config ./config/orch.toml` prints only the effective primary for local scripts.

Doctor reports the file seed and stored address separately and probes the effective browser page and API. `--url` selects an explicit diagnostic route. Updater readiness reads the effective address on every attempt; `ORCH_READY_URL` remains an explicit operator route.
