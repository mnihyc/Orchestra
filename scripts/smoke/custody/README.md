# Storage checks

`python3 scripts/smoke/custody/releases.py` checks the actual updater release-pruning function on isolated files.

`python3 scripts/smoke/custody/uninstall-stack.py /path/to/orch-node /path/to/orchctl /path/to/artifacts`
runs the actual stack uninstall command with preservation, purge with a final backup, and explicit backup omission.
Use current executables. Native GNU executables need to run on the stock Trixie images; released static executables
also work. The script creates private `orchcustody-*` Compose projects with PostgreSQL 17. No model service runs.
It checks another system's sentinel and data throughout, then removes its own Docker objects. Fixture files and a
complete log remain in the artifact directory for inspection.

The node and connector integration tests named `uninstall` execute copied installed programs. They check preview,
confirmation, shared programs, retained identity, pinned folders, borrowed data and actual Docker inventory removal.
