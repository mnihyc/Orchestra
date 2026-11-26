# update-doctor scenario

`./update.sh doctor` runs the two readiness checks of the stack once, and the wait of an update (`wait_ready`) builds the
same two commands. This scenario shows both with no stack, no Docker and no release: it runs a copy of `deploy/update.sh` in a
directory of its own against stand-ins for `docker` and for `orchctl` that print what they were asked, and prints for each
promise `PASS` or `FAIL`.

1. **The address.** The edge check goes to `ORCH_READY_URL` when the environment sets it, else when `.env` does, else to
   `[system] public_url` of `config/orch.toml`; the first line says which, and no address at all fails with what to set.
2. **The commands.** The check inside the API's container leaves the node out where the Compose file has none and never
   where it has one (forty tries: the list of services is read whole, not by a reader that leaves at its first match); the
   edge check asks for the three edge checks only, with the config and the secrets directory.
3. **A failure.** A failing inside check, a failing edge check and a host without `orchctl` each make `doctor` exit non-zero
   with their own lines printed; the second check runs whatever the first says.
4. **Nothing changed.** No journal and no state are written, nothing is started, stopped, built or pulled, and an argument is
   refused. The usage block lists `doctor`.
5. **The wait of an update.** `wait_ready` passes when the edge serves the release it expects, asks the inside check to expect
   that release, and does not pass while the edge serves another.

The stack-side half is in [first-start](../first-start/README.md): `update.sh doctor` passes on a ready stack, names the
address and its source, changes nothing, and fails naming the services that were stopped.
