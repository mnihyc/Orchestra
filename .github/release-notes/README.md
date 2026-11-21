# Release notes

Each release has a file `<tag>.md` here, in the form of [TEMPLATE.md](TEMPLATE.md): upgrade notes, breaking changes, highlights, changes, known issues, compatibility and downloads, in that order. The release workflow refuses a tag without its file and publishes the file as the release description.

While a release is being made, its file holds what the release has so far, and [CHANGELOG.md](../../CHANGELOG.md) holds the same text under "Unreleased". Lines marked "In progress" are not done: finish them or delete them before the tag is made. At the tag, "Unreleased" becomes the release's own entry.

From the release after v0.1.3 the notes give the SHA-256 of that release's `update.sh` on a line of its own,
`update.sh SHA-256: <hex>`; the gate compares it with `deploy/update.sh`. When the release raises
`minimum` in `deploy/FILE_SETS`, the upgrade notes say which files changed and why.
