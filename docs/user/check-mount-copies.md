---
title: Choose and check mount copies
screens: /thread/$threadId, /resources/environments/$id, /resources/mounts, /resources/mounts/$id
keywords: mount, copy, synced, own data, shared storage, fingerprint, content check, same content, differs, detach, replace, keep both
---

# Choose and check mount copies

## Choose what follows

A mount's Own data, Shared storage or Synced copy label is a hint from its
manager. It does not copy, synchronize or change the mount's permissions.

A move, fork or environment-switch plan lists each mount. Choose **Detach**,
**Mount as-is** or **Copy** from the choices actually available. The mount's
default is preselected; **Mount as-is** is the shipped default. **All mounts** changes the common choice; each row
can still override it. An unavailable choice explains its reason.

Mount as-is uses the destination's location. Copy transfers the selected data
before work starts there. If that location already holds data, choose **Replace**
or **Keep both**. Keep both copies into a new location. The plan states what is kept and the retention before
confirmation. Read the size, pending counts and required authority.

## Content check

For an eligible read-only Synced copy used as-is, **Check** compares content
fingerprints at the source and destination. It reads both locations only when
you ask. It does not synchronize or change them. A fresh result says **Same
content** or **Differs**, with the check time.

Checking shows source then destination progress in that mount's row. Use
**Stop checking** to end the read. Stopping waits for the actual machine work;
a stopped or failed check has no comparison result. Retry starts a fresh check.

A result is an observation at its displayed time. A later file change can
invalidate it. Changing the plan's choices drops the old observation. A
different fingerprint does not change your mount choice or grant permission.
If Check is absent, that plan or mount does not offer this comparison.

## Review tool servers too

The same plan lists what happens to MCP servers: kept, lost as a machine import,
unavailable with a reason, or newly available. A mount copy does not copy a
machine's private imported server or its secrets. Review both lists before
starting the operation.
