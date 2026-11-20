# Security policy

## Report a vulnerability

Please report a vulnerability privately. Do not open a public issue.

Use GitHub's private reporting: [report a vulnerability](https://github.com/mnihyc/Orchestra/security/advisories/new). Only the maintainers see the report.

Tell us:

- what you found and where (the component and the Orch version, from `./update.sh status`);
- how to reproduce it, or a short demonstration;
- what an attacker could do with it, and who would be affected;
- anything you changed in the default configuration.

What happens next: we acknowledge the report, tell you what we find, and fix it. When the fix is released we publish an advisory and credit you by name if you want that. Please give us the time to fix it before you tell others, and please do not use a vulnerability to read or change data that is not yours.

## Supported versions

Orch is pre-release (0.x). Fixes are made in the latest release only; update with `./update.sh latest`. Older releases get no fixes.

## Securing your installation

How Orch decides who may do what, what it protects and what it does not is in the [security model](docs/security/model.md). Run Orch behind HTTPS, keep `deploy/.env` and `deploy/config/secrets/` readable by their owner only, and keep the host's Docker socket away from everything but the node: [Secure an installation](docs/operator/security.md) is the checklist.
