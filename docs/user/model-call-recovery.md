---
title: Model call recovery
screens: /thread/$threadId, /topic/$topicId, /project/$projectId/settings, /org/$slug/settings/$section, /admin/$section, /inbox, /usage, /settings/usage
keywords: call recovery, retry, continue, failover, provider busy, request refused, access not granted, repeat limit, summary, description, 调用恢复, 重试, 继续, 故障转移, 摘要
---

# Model call recovery

## Set rules

Open **Call recovery** in a place's settings. Rules are inherited from the system, organization,
project or folder, then topic. An inherited lock prevents a lower place from changing the value.
The first matching rule handles the failure. Put a specific rule before a general one.

Open a row to edit its conditions and ordered steps. Use **When**, **Reason**, **Purpose**,
provider and model to narrow it. **Words contain**, **Regular expression** and **HTTP status**
can match a vendor message that needs its own handling. Text matching ignores case.
Use the row menu to reorder or remove rules and steps. Save applies the rule; Cancel leaves it unchanged.

The shipped rules retry **Provider busy** and **Provider unreachable** up to three times.
The first pause is one minute, with a pause multiplier of two. A vendor's retry time takes priority.
**Request refused** continues up to three times. **Limit reached** waits for a known reset.
**Access not granted** and other failures wait for a person. These are editable defaults.
An empty rules list leaves failures for a person.

## Choose candidates

Add **Fail over** to a rule and choose a provider, model and, when needed, agent profile.
The choices use the same usage and load cards as other model choices.
Only a candidate in the rule can run. Orch does not choose an unlisted provider.
The person whose work is being recovered must still be allowed to use the candidate when it runs.
An unavailable or disallowed candidate is skipped with a reason.

**Parameters** is an object of the model or agent's supported options. It cannot grant access or
enable a feature that the place forbids. A candidate for embeddings must keep the same model and
embedding dimensions. Changing either produces incomparable vectors, so that candidate is skipped.

A model change on the same provider and agent profile keeps the session where supported.
A different provider or profile uses the ordinary switch plan and carries the selected history.
A blocked switch needs attention. **After failover** defaults to returning to the thread's own choice
at the next turn when it is ready. Choose **Stay on candidate** to keep the candidate instead.

## During recovery

The thread's live line names the failure, next action, known wait and attempt count.
The model chip shows the model that actually runs. A repeated failure uses one error line with its attempt count.
Each automatic step leaves one history line with its rule, recorded attempt limit and time.
A known wait in a switch job appears in that job's progress line too.

**Retry now** brings a scheduled attempt forward. It still checks current access and limits.
**Stop retrying** stops future automatic attempts. It does not undo work already done.
An accepted switch keeps its own Cancel control. Stopping retries during that switch prevents a
follow-up input and keeps the original-choice return policy.
When attention is needed, the banner offers **Retry**, **Continue** and **Switch**.
Retry and Continue require Interact. Switch requires thread management access.
While automatic recovery is scheduled, its live controls replace the failed turn's manual actions.
A turn followed by another turn has no retry button. A stopped recovery keeps its reason and actions
in the attention banner.
Someone who can only view the thread can still read the reason and this help.

A turn stopped because its provider, model, agent profile or environment was switched off is not a
model failure. Recovery rules do not retry it or send an automatic Continue. When enabling the resource,
**Run the waiting messages** also resumes that stopped work through a visible `Continue` by its original
author. With that choice off, the queues stay held until someone uses Continue or Resume.

## Stop repeated failures

**Repeat limit** and **Repeat window** bound automatic recovery of the same failure kind in a thread.
The defaults are three recoveries in five minutes. Exceeding the limit stops automatic handling,
including a turn that kept a partial answer. The banner shows the kind, count and window.
Set Repeat limit to **Off** to stop automatic recovery altogether.

**Notify on stop** is off by default. When enabled, a repeated failure sends the thread owner one
grouped Inbox notice. Open the notice to inspect the thread and decide what should happen next.
Rules, the guard and the notice follow the same inheritance and locks as other place settings.

## Automatic work and access

If the failed turn kept no answer and started no tool, recovery can run the original input again.
If it kept an answer or started a tool, recovery adds a visible `Continue` input instead.
It names the original input's author. Its header says **Sent automatically by a rule**.
The tooltip names the rule and attempt. The history line explains why it continued and links to the rule.
Its attempt limit is the value recorded when it ran; later rule edits do not change that history.
The person who wrote the rule is not its sender.

Automatic work retains the original input's recorded submission and approval boundaries.
Orch checks that authority again before acting. A revoked token or session stops automatic work;
an open browser with broader access cannot replace it. Every served attempt is counted to the person
whose work it serves. Usage records name the provider and model that served that attempt.

## Summaries and descriptions

Summaries, handoff summaries and automatic descriptions use the same rules with **Purpose: Summary**.
Their calls use the current access of the setting's writer. Recovery keeps the same work to summarize
and records each model call separately. A handoff summary's known wait appears in the switch job.
If its chain cannot continue, the switch uses its normal transcript fallback.

A stopped automatic summary or description shows **Call recovery needs attention** in its setting.
Read the reason, fix the rule, access or model choice, then request the automatic value again.
A missing reset time needs a person; Orch does not invent a deadline.
An automatic value whose provider or model is switched off waits with that reason. Switching it off
does not count as a failed attempt, and no model request is retried against it while it is off.
Review rules apply to model calls Orch controls. Reviews performed inside an agent program also
follow that program's own controls.

See [Settings and usage](settings-and-usage.md) for inherited values and consumption,
and [The message queue](message-queue.md) for work waiting to reach the agent.
