---
title: Give agents versioned skills
screens: /resources/skills, /resources/skills/$id, /thread/$threadId, /org/$slug/settings/$section, /project/$projectId/settings, /admin/$section
keywords: skills, SKILL.md, archive, import, git, version, draft, publish, latest, pin, agents.skills, scripts
---

# Give agents versioned skills

## Import a skill

Open Resources → Skills → Add skill. Choose an archive, a Git repository,
an address, an environment directory or a new draft. Choose its home. A skill
contains `SKILL.md` and may include references, scripts and other files.

An archive can select a folder inside it. Git and address imports use the host
you choose; Git also takes a revision and optional subfolder. A directory import
uses an environment you may manage. These acts copy a version into Orch;
they do not bind the skill to that environment for later use.
Ask your system operator about access to private sources.

Treat scripts as executable content. Review the source and files before making
the skill available. Private source credentials stay sealed; reading the skill
does not return them.

## Review and publish

Overview shows the latest version, origin, author, time and size. Versions
opens earlier immutable versions and their files. Text files open read-only;
binary files keep their file metadata.

Use **Update from origin** to import again, or **Add version** for another archive
or source. An unchanged source says it is up to date. For an edit, open Draft,
start from the current version, save text files or upload replacements, and
set the executable bit for scripts where needed. Publish makes a new version;
Discard removes the draft. An unpublished draft does not reach other threads.

## Enable published versions

In a place's settings, Skills (`agents.skills`) adds a skill you may use.
Choose Latest to follow published updates, a numbered version to pin it, or
Off to disable that entry. Inheritance and locks work like other settings.
The separate personal-skills policy decides whether starters may add their own.

The skill's Access page separates Use from Manage. Use allows enabling an
available skill; Manage allows changing its content and versions. Viewing a
resource alone grants neither. Disabling the skill stops its availability.

## Know when a change arrives

Published enabled versions reach a thread before its next turn. An idle
session can refresh in the background; an arriving input says **Updating
skills** while that refresh finishes. Each turn records the versions it used.

Delivery belongs to the thread's session and command environment. Orch keeps
skills out of a provider's shared home. Enabling a skill grants no additional
file, network, provider or tool permissions; its work follows the thread's
ordinary boundary and approvals.
