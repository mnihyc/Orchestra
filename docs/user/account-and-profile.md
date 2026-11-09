---
group: access
title: Manage your account and profile
screens: /login, /register, /recover, /reset/$token, /verify-email/$token, /invite/$token, /settings, /settings/profile, /settings/your-page, /settings/security, /settings/tokens, /settings/exports, /settings/interface, /settings/notifications, /settings/usage, /u/$username
keywords: account, profile, sign in, password, recovery, email, authenticator, sessions, API token, exports, interface, personal settings
summary: Manage your profile, sign-ins, account recovery, sessions and personal choices.
---

# Manage your account and profile

## Sign in and recover access

Use the sign-in methods offered by your installation. Registration and
invitations follow its policy. Verify email through the link sent to you;
an expired or already used link says what to do next. Email verification
does not sign an agent in to a model provider.

Use Recover access when you cannot sign in. A reset link is single-use and
expires; follow its form without sharing the link. If you cannot complete
the second step, ask your installation administrator for its reset procedure.

## Set your profile

Settings → Profile edits your name and profile details. Your page holds the
words and pins people see when opening your public profile. People see only
the pinned work and activity their current access permits; a pin grants no
access to its target.

## Protect sign-in

Settings → Security manages your sign-in methods, second step, recovery codes
and signed-in browsers. Complete the offered setup before relying on a new
method. Keep recovery codes in a place you control. Sign out another browser
from the session list when it should no longer use your account.

Manage automation keys in Tokens. Name the key, choose its targets and resource
kinds, then review the exact operations selected by Read-only, Write-only,
Read and write, Full management or Custom. Each rule keeps its operations bound
to its own target. Presets save the reviewed selection; later API additions do
not expand existing keys. Choose an expiry and copy the secret when it is created.
Later lists show its policy and recent activity. Revoke keys you no longer need.

A program uses the secret with `Authorization: Bearer <key>` on the same API.
The key is limited by your current authority and its own rules. Administrative
operations need your current administrative standing and an explicit selection;
the browser's As admin private-access override is not delegated. A write-only
operation returns an acknowledgement without existing content. A newly issued
secret remains available once in that acknowledgement.

Recent activity shows up to 100 calls from the last seven days, with operation,
target, status and timing. Resource names and source addresses follow the reader's
current access. Request bodies, response contents and secrets are not kept there.

## Keep personal choices

Interface sets language, theme and shortcut preferences. Help → Shortcuts is
the binding editor, including when single-key shortcuts are off. Notifications
sets your routes and exceptions. Usage opens your consumption; Exports keeps
the export jobs and downloads offered to your account. A failed job keeps its
reason and does not mean a download is ready.

These personal choices do not edit another person's layout or a place's
settings. For inherited values and consumption, read
[Settings and consumption](settings-and-usage.md).
