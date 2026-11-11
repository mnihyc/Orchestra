---
group: access
title: Read notices and send announcements
screens: /inbox, /settings/notifications, /org/$slug, /project/$projectId, /topic/$topicId, /thread/$threadId, /org/$slug/settings/$section, /admin/$section
keywords: inbox, notice, announcement, milestone, delivery, wake, unread, mentions, bell, notification
summary: Read Inbox notices, choose notification delivery and send announcements with visible outcomes.
---

# Read notices and send announcements

## Read your Inbox

Inbox keeps what Orch told you. Choose All, Unread or Mentions, and group by
day or place. Open a row to see its work. Mark it read, or use Undo to return
the previous read state. Older rows load through the page controls.

An Inbox row does not grant access to the original work. A changed permission
can remove its private detail. Notification settings decide outward delivery;
marking work read and pausing delivery have different effects.

Messages in the left bar shows your unread chat count for the current root
organization. The organization in the rail uses the same count for its dot.
Both update from your personal chat state even while Chats is hidden in the
tree. Hidden or unfollowed chats contribute nothing; reading in another browser
updates the count here too.

## Choose when to be told

Use a place's bell for that place's level. Default follows the nearest choice
above it. Settings → Notifications lists your exceptions, quiet hours, pause
and outward routes. A route works only after its own setup; this browser's
push permission applies to this device. Read
[Notifications](settings-and-usage.md#notifications) for delivery and retention.

## Send an announcement

Open Announcements from an organization, project or topic. Choose Notice or
Milestone, its importance and whether it targets all or active threads. Write
the title and body. You need the place's participation permission, and an
archived place cannot send.

Review delivery before sending. Next turn waits; Steer joins an active turn
when supported; Interrupt stops that turn before delivery. Default takes the
place's setting. The fallback decides what happens when steering is unavailable.
Start idle threads is a checkbox, unticked for each send. Tick it only when you
want this announcement to start a turn. The `broadcast.wake_idle` setting allows
that choice and ships on; switching it off disables the checkbox with a reason.
Leaving the checkbox unticked keeps idle threads waiting for their next turn.
These choices affect agent work, so review the audience and delivery together.

Agents also choose `wake_idle: true` explicitly for `announce` or a thread-targeted
`send_message`; omission means false. A true request where the receiving topic
forbids wake is refused and names `broadcast.wake_idle`. A send across several
topics is refused as a whole if any receiving topic forbids the requested wake.
The shipped hourly limits are 12 announcements (`broadcast.rate_per_hour`) and
60 individual thread messages (`broadcast.thread_messages_per_hour`), separately
for each sending thread. Zero forbids that kind of send. Follower-only milestones
use neither limit.

## Follow the result

The result lists delivered, queued and skipped work. Recent announcements
keeps the title, body, delivery counts and time, including a just-accepted send.
Page back for earlier entries. A failed send keeps its reason; check that
result before submitting the same announcement again.

Announcements and agent milestones stay recorded in the conversation. An
announcement to agents and an Inbox notice to people have different recipients.
