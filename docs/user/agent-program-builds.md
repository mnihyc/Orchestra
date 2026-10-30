---
title: Agent program builds
screens: /admin
keywords: harness, agent, program, build, checksum, mirror, offline, vendor, trust, 构建, 校验和, 镜像, 离线
---

# Agent program builds

System administrators manage builds in **Management → Agent programs**. A build's checksum source and
its check result answer different questions: who supplied its expected bytes, and whether its
compatibility check passed. Making a build the default keeps both facts.

## Checksum sources

Release builds take their checksums from the Orch release. Vendor-channel builds use checksums
read directly from the vendor. An administrator may add an expected checksum, or import a vendor
checksum list carried in from a connected machine. Imported builds start **Untested**.

A normal mirror carries program files. It cannot establish an expected checksum by serving its
own metadata. For an internal network, gather the vendor checksums outside and explicitly import
them inside. The import command shows the list's digest and records it in the administrator audit.

## Trusting a mirror

**Trust mirror checksums** is a system setting and is off initially. Enable it only when that
mirror is allowed to publish executable builds. Such rows say **Checksum from mirror (not the
vendor's)**. A passed compatibility check keeps that label. Turning trust off makes those builds
ineligible for selection and background checks.

Older records with no established checksum origin are unavailable until an administrator imports
their vendor list or explicitly adds their expected checksum. The operator guide at
`docs/operator/offline.md` explains gathering, importing and serving the files internally.
