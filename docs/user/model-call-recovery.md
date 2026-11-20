---
group: work
title: Model call recovery
screens: /thread/$threadId, /topic/$topicId, /project/$projectId/settings, /org/$slug/settings/$section, /admin/$section, /inbox, /usage, /settings/usage
keywords: call recovery, retry, continue, failover, provider busy, request refused, access not granted, repeat limit, summary, description, 调用恢复, 重试, 继续, 故障转移, 摘要
summary: Set rules for what happens when a model call fails, watch a recovery, and stop repeated failures.
---

# Model call recovery

When a model call fails, Orch follows the first rule that matches: it waits and tries again, continues, switches to another provider or model, or leaves the failure for a person. This page shows how to set those rules, what a thread shows while one runs, and how to stop a failure that keeps coming back.

## Set rules

1. Open the settings of a place and choose **Call recovery**.
2. Open a rule's row to edit it, or add one.
3. Narrow the rule with **When**, **Reason**, **Purpose**, provider and model. To catch a provider's own message, use **Words contain**, **Regular expression** or **HTTP status**. Text matching ignores case.
4. Add the steps in the order they should run. The row menu reorders or removes rules and steps.
5. Choose **Save**. **Cancel** leaves the rule as it was.

Rules are inherited from the system, then the organization, the project or folder, and the topic. A place that inherits a locked value cannot change it. The first matching rule handles the failure, so put a specific rule before a general one. An empty list leaves every failure for a person.

The default rules, which you can edit:

- **Provider busy** and **Provider unreachable**: try again up to three times. The first pause is one minute and each next pause doubles. A retry time that the provider gives takes priority.
- **Request refused**: continue up to three times.
- **Limit reached**: wait for the reset time, when it is known.
- **Session lost**: continue a running session once, when its machine is reachable again.
- **Access not granted** and every other failure: wait for a person.

## Choose candidates

To switch to another provider or model, add **Fail over** to a rule. Choose a provider, a model and, when needed, an agent profile. The choices show the same usage and load cards as other model choices.

- Only a candidate listed in the rule can run. Orch never picks a provider that the rule does not name.
- The person whose work is being recovered must still be allowed to use the candidate when it runs. A candidate that is unavailable or not allowed is skipped, and the thread says why.
- **Parameters** takes the options that the model or the agent program supports. It cannot grant access or turn on something the place forbids.
- A candidate for embeddings must keep the same model and the same vector size. Otherwise its vectors cannot be compared with the old ones, and the candidate is skipped.

A model change on the same provider and agent profile keeps the session where the agent program supports it. A different provider or profile uses the normal switch plan and carries the history you chose. A switch that is blocked needs attention.

**After failover** sets what happens next. By default the thread goes back to its own choice at the next turn, when that choice is ready. Choose **Stay on candidate** to keep the candidate.

For a summary or a review call, the candidate keeps the request format of the original call when the target offers it, and otherwise uses the target's completion format. The candidate editor shows the format when the context fixes it. A generic rule can serve calls of different formats: it says **From the original request** and shows **Raw JSON** until you choose a preview. Choosing a preview changes the editor only; it adds nothing to the saved rule.

## During recovery

The thread's live line names the failure, the next action, the wait when it is known and the number of attempts. The model chip shows the model that actually runs. A failure that repeats uses one error line with its attempt count. Each automatic step leaves one history line with its rule, the attempt limit recorded at that time, and the time.

When the machine of a lost session is offline, the live line says it is waiting for the machine to return. The attempt stays reserved if Orch restarts, and **Stop retrying** still holds it.

What you can do:

- **Retry now** brings a scheduled attempt forward. It still checks your current access and limits.
- **Stop retrying** stops future automatic attempts. It does not undo work already done. An accepted switch keeps its own **Cancel**. Stopping retries during a switch prevents a follow-up input and keeps the return to the original choice.
- When attention is needed, the banner offers **Retry**, **Continue** and **Switch**. **Retry** and **Continue** need Interact access to the thread. **Switch** needs Manage access.
- While the thread waits in call recovery, use its live controls or the banner. The **Retry** of the failed turn stays visible but disabled, with the reason. When the wait ends, a failed or interrupted turn can be retried from the history. **Retry** on an earlier completed turn stays disabled.
- A stopped recovery keeps its reason and its actions in the banner. Someone who can only view the thread can still read the reason and this page.

A turn that stopped because its provider, model, agent profile or environment was turned off did not fail, so recovery rules do not retry it or send **Continue**. When you turn the resource on again, **Run the waiting messages** resumes started work through a visible **Continue** from the original author. A turn that had not begun runs its original input. With that choice off, the queues stay held until someone uses **Continue** or **Resume**.

## Stop repeated failures

**Repeat limit** and **Repeat window** bound how often the same kind of failure is recovered automatically in one thread. The default is three recoveries in five minutes. Past the limit, automatic handling stops, including for a turn that kept a partial answer. The banner shows the kind, the count and the window. Set **Repeat limit** to **Off** to stop automatic recovery altogether.

**Notify on stop** is off by default. When it is on, a repeated failure sends the thread's owner one grouped Inbox notice. Open it to look at the thread and decide what to do. The rules, the limit and the notice follow the same inheritance and locks as other settings of a place.

## Automatic work and access

If the failed turn kept no answer and started no tool, recovery runs the original input again. If it kept an answer or started a tool, recovery adds a visible **Continue** input instead. The loss of a running session also uses **Continue**, because the program may have done work it could not report. If the session never confirmed that the turn began, a person decides what to do with the waiting input.

- The input names the original author as its sender, and its header says **Sent automatically by a rule**. The tooltip names the rule and the attempt, and the history line explains why it continued and links to the rule.
- Its attempt limit is the value recorded when it ran. Later edits of the rule do not change that history.
- The person who wrote the rule is not the sender.

Automatic work keeps the limits that the original input was sent with, and Orch checks the author's access again before it acts. A revoked token or sign-in stops automatic work; a browser that is open with broader access cannot replace it. Every attempt that was served counts to the person whose work it serves, and the usage records name the provider and model that served it.

## Summaries and descriptions

Summaries, automatic titles and tags, and handoff summaries use the same rules with **Purpose: Summary**. Their calls use the current access of the person who wrote the setting. A recovery keeps the same work to summarize and records each model call separately. The wait of a handoff summary shows in the switch job. If its chain cannot continue, the switch falls back to the normal transcript.

A summary, title or tag update that stopped shows **Call recovery needs attention** in its setting. Read the reason, fix the rule, the access or the model choice, then ask for the automatic value again. A missing reset time needs a person: Orch does not make up a deadline. An automatic value whose provider or model is turned off waits with that reason. That is not a failed attempt, and no request is sent to it while it is off.

These rules cover the model calls that Orch makes. A review that an agent program performs inside itself also follows that program's own controls.

## Capability fallbacks

The same rules cover **Web search**, **Page fetch** and **Image generation**. **Capability unavailable** means that the current agent program, sign-in, model or turned-on services cannot supply the capability. **Program default** keeps the program's own behavior and adds no hidden service or fallback.

1. Under **Purpose**, choose the capability.
2. Add **Fail over** and choose **Capability service** as the destination.
3. Choose the provider, choose how it runs the capability, and name the model or service.
4. Add the steps that follow, for example **Wait for a person**.

A model tool through Orch, a provider API and a session of another program are all explicit candidates of the same rule. Before it uses a candidate, a call checks current access, the services that are turned on and the limits. No paid fallback rule is set by default.

For example, set a rule for **Capability unavailable** with **Purpose: Page fetch**. Choose the provider's page-fetch service as the first candidate and **Wait for a person** as the next step. The thread's **Parameters** view previews the rule and where it applies. A provider you cannot read stays unnamed. A candidate that cannot be used is skipped with its reason, and the next step goes on. If a candidate fails for another cause, the chain goes on and records that cause. A finished call does not change the original choice of the next, independent turn unless **After failover** says to stay.

A longer chain can try again twice, then one named image service, a second image service and a final wait. When the chain ends, the thread stops with **No recovery steps remain**. The repeat limit and the current permissions still apply. A capability that is unavailable is never permission to look for another provider or to charge one.

## Paused web services

A managed **Web search** or **Page fetch** call ends at once when its rule requires a person, or when the reset time it reports lies beyond that call's deadline. The result tells the agent that the service is paused, why, and the reset time when it is known. It does not claim an empty search or a finished page read. Only the work measured before the pause counts.

The thread's recovery line names the paused service. **Resume service** clears the pause for the next call. It does not rerun the closed request and adds no input. At a reported reset the pause ends by itself, and the next call goes through the usual checks of access, policy and limits. A pause that waits for a person stays until someone with Interact access to the thread resumes it. There is no control for an old single tool request.

See [Settings and usage](settings-and-usage.md) for inherited values and what a place consumes, and [The message queue](message-queue.md) for work waiting to reach the agent.
