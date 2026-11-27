# Orch documentation

Orch is a self-hosted workspace where a team runs coding agents together. The documentation has four parts, by what you want to do. Start with the first page of the part you need.

## Use Orch

Start with [a team working beside an agent](user/getting-started.md): one person starts the fix, a colleague reads and answers the waiting request, and both continue the same thread while they talk in the chat. The [user guide](user/README.md) explains the screens and common work, and it is also the help inside the application.

## Run Orch

Start with [Install Orch](operator/deployment.md). The [operator guide](operator/README.md) covers the HTTPS front, configuration, updates, backups, monitoring and recovery. To report a vulnerability, see the [security policy](../SECURITY.md). The [security model](security/model.md) describes the controls and their limits, and [Secure an installation](operator/security.md) says what you do to keep them.

To call the API from a script, see [Use the API with a key](operator/api-with-a-key.md). [`openapi.json`](openapi.json) describes the key API.

## Extend Orch

Start with [Extend Orch](development/extensions/README.md): what each kind of extension is, which ones have an implementation, and how to write and check one.

## Contribute

Start with [CONTRIBUTING.md](../CONTRIBUTING.md). The [architecture](development/architecture.md) explains ownership and boundaries, the [component inventory](development/components.md) maps every package and feature area, [Writing for Orch](development/writing.md) sets the rules and the glossary for documentation, screen text and release notes, and [Publish a release](development/releasing.md) describes the release process.

What each release changes, and what you must do to update, is in the [changelog](../CHANGELOG.md).
