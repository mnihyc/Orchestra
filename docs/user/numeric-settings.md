---
group: settings
title: Zero in settings
screens: /settings, /resources, /project/$projectId/settings, /org/$slug, /topic/$topicId, /thread/$threadId, /admin
keywords: zero, 0, Off, numbers, days, grace, wait, tags, preview, inherited settings
summary: Understand when zero means Off, an immediate action or an ordinary numeric value.
---

# Zero in settings

## Zero values

When zero switches a behavior off, its field shows **Off**. Focus the field to
edit the number. Enter a positive value to turn that behavior back on. Saving
still follows the setting's permission, inheritance and lock.

Other zero values keep their numeric meaning. Read the question mark beside
the setting before changing it:

- **Removed files:** zero makes files eligible for removal when their unit is
  removed. It does not shorten an existing recovery promise.
- **Recycle idle time:** zero applies no minimum idle time.
- **Recreate grace:** zero interrupts running turns immediately.
- **Tags on a place:** zero allows no tags.
- **Wait:** zero does not wait for running turns.
- **File preview context:** zero shows the referenced line without extra lines.

Choose **Here** to edit an inherited number. Choose **As Acme decides** (named
for its actual source) to remove your local choice and follow that value again.
**Reset to inherited** in the row menu does the same thing. Entering zero is a
different choice.

For automatic retention, read [Storage cleanup](storage-cleanup.md).
