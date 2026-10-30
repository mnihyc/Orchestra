# orch-agent-programs

Agent program manifests and the registry of the built-in programs.

A program manifest is the static data Orch needs about an agent program before anything runs: what it is called in each language, which protocol adapter drives it, how it is launched, where its builds are published and how they are verified, what its build checks look for, which executors it brings, which files it reads, and what it does to keep itself quiet. The descriptor an adapter declares stays what a running build proves; nothing is written in both. The registry holds the manifests of the programs Orch ships, as data files in this package, and every list of built-in programs reads it: the connector's composition, the seeded profiles, the plugin-name collision lists, the colocated default, the build checks and the release channels. A program that Orch does not ship sends the same manifest when its adapter plugin says hello.

| Module | Planned responsibility |
|--------|------------------------|
| [localized](src/localized.rs) | Words in the languages the interface speaks, with the English text as the fallback. |
| [catalogue](src/catalogue.rs) | The words for what a check of a build reports: its cases and the features it switches off, in each language. |
| [manifest](src/manifest.rs) | The program manifest: what Orch needs to know about an agent program before anything of it runs. |
| [channels](src/channels.rs) | Where a program's builds are published, and how each build is verified. |
| [executors](src/executors.rs) | The manifest of an executor: the program that carries out a program's tool calls inside an environment. |
| [registry](src/registry.rs) | The registry: the manifests of the programs Orch ships, and the lookups every list of programs reads. |
