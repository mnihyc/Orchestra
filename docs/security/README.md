# Security

Start with what you need.

## Report a vulnerability

Please report it privately and do not open a public issue. Use GitHub's private reporting: [report a vulnerability](https://github.com/mnihyc/Orchestra/security/advisories/new). What to include, and what happens next, is in the [security policy](../../SECURITY.md).

## Supported versions

Orch is pre-release (0.x). Fixes are made in the latest release only. Update with `./update.sh latest`; older releases get no fixes.

## Read about the controls

- **You run Orch:** [Secure an installation](../operator/security.md) is the checklist of what you do and what you leave alone: the HTTPS front, the secret files, backups of the keys, the Docker socket, the images and the network.
- **You decide whether to trust Orch, or you administer it:** the [security model](model.md) says what each control guarantees and where it stops: access and admins, threads and their limits, environments, machines, providers and credentials, what agents can reach, documents, shells and files, signing in, notifications, and plugins.
- **You are on a team that uses it:** [Who can see and do what](../user/who-can-see-and-do-what.md) in the user guide describes the same controls from the screens.
