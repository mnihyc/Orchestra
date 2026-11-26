# update-files scenario

The files of the release's `orch-deploy` bundle follow the release (see [Upgrades](../../../docs/operator/upgrades.md#deployment-files)).
This scenario shows what `update.sh` decides, writes, records and puts back, with no stack, no Docker and no release: it reads
`deploy/update.sh` into a shell in a directory of its own, against a stand-in for `docker`, and calls the functions of the file
logic with bundles it makes, so that what is decided, written, kept and left behind can be looked at. What a real transaction does
with them (stop, dump, switch, start, verify, roll back, recover) is in the [update](../update/README.md),
[rollback](../rollback/README.md) and [recover](../recover/README.md) scenarios.

1. **Published files are replaced, as a set.** Files that equal a version a published release shipped are replaced by the new
   release's, one line each; the earlier ones are saved; `.env`, `config/` and the secrets are untouched; the updater is replaced
   only when the update has finished, in one rename, and keeps the bundle's mode.
2. **An edit keeps the set.** An edited member keeps its whole set; each member that differs gets the shipped file beside it as
   `<name>.new`, once: after the `.new` is removed it is not written again for the same shipped file, and is for a later one. Other
   files still follow. `status` lists a kept file until its `.new` is gone.
3. **The oldest generation the payloads work with.** A kept set older than the release's `minimum` stops the plan, before anything
   is stopped, naming the `.new` files to merge; once they are merged and removed, the update goes on. An old set that is not kept
   is replaced, so it is not too old. A release without its bundle follows no file, says so, and is still checked against the
   generation the last update recorded.
4. **Guards.** A new `postgres/Dockerfile` of another PostgreSQL major than the data directory's (R1), a `compose.yml` that stops
   binding a directory of `runtime/` or `config/` (R2), and an image the host does not have and cannot pull, or that is not among
   the carried-in ones (R3), each keep the set and say in one line what was found and what to do; an image that can be pulled is
   pulled during the plan.
5. **The updater is its own.** An edited `update.sh` is kept with `update.sh.new`; one newer than the release is left alone.
6. **Rolling back.** The files the last update replaced are put back where they are still what it wrote; a file edited since stays,
   and the command says so; a file the update added is removed; `update.sh` is not rolled back; the generation goes with the files;
   a second rollback goes forward again.
7. **Recover.** A transaction cut off while the files were written puts every file back, one edited since stays and is named, and a
   changed `compose.yml` makes the database's container again (only then).
8. **A script replaced while it runs** by a rename, or written over in place, finishes as the version that started; the last line
   of `update.sh` calls `main` and exits.
9. **The first hop.** The command in the upgrade guide, with its placeholders filled, refuses a wrong digest and an unfinished
   update, and installs the updater when the digest is the file's.

Run it with `bash scripts/smoke/update-files/run.sh`.
