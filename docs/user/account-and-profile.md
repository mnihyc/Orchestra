---
title: Manage your account and profile
screens: /login, /register, /recover, /reset/$token, /verify-email/$token, /invite/$token, /settings, /settings/profile, /settings/your-page, /settings/security, /settings/tokens, /settings/exports, /settings/interface, /settings/notifications, /settings/usage, /u/$username
keywords: account, profile, sign in, password, recovery, email, authenticator, sessions, API token, exports, interface, personal settings
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
Removing a second-step method clears the second-step approval in every browser.
Each browser must give a current method again where a second step is required.

API tokens are managed in Tokens. Name a token, choose its permissions, place
and expiry, and copy its secret when it is created. Later lists show metadata,
not that secret. Revoke tokens you no longer need. A token is limited by your
current access and never carries browser-only administrator authority.

## Keep personal choices

Interface sets language, theme and shortcut preferences. Help → Shortcuts is
the binding editor, including when single-key shortcuts are off. Notifications
sets your routes and exceptions. Usage opens your consumption; Exports keeps
the export jobs and downloads offered to your account. A failed job keeps its
reason and does not mean a download is ready.

These personal choices do not edit another person's layout or a place's
settings. For inherited values and consumption, read
[Settings and consumption](settings-and-usage.md).
