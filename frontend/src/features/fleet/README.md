# Fleet: environments, provider runtimes and recycling

Selected environments and provider runtimes share one reviewed update dialog. It shows skipped units, target builds and their checks, running turns, a shared wait or interrupt choice, optional notices and each job's progress. The resource lists, host runtimes and program-version page open the same dialog; individual environment changes and recycling remain in [change](../change/README.md).

- `EnvironmentTable.tsx`: Environments with their owner and threads, the shells open in each, activity, capacity, pending updates and controls.
- `ProviderRuntimeTable.tsx`: Provider runtimes beside the environments: provider, host, template, state, pending updates and controls.
- `MoveDialog.tsx`: Move selected environments to another host, with one reviewed plan each.
- `UpdateDialog.tsx`: Update selected environments and provider runtimes, with one reviewed plan each.
