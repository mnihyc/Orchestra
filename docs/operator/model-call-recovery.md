# Set the rules for failed model calls

Choose what Orch does when a model call fails: wait and try again, continue, switch to another provider or model, or leave it to a person. At the end every failure kind has a rule that suits your installation, and you can tell from a thread why a call waited or stopped.

Recovery is policy of a place. Set it at the system, an organization, a project or folder, or a topic. The usual inheritance and locks apply, and a lower place cannot bypass an ancestor's lock. People set rules, read a recovery and stop a retry from the screens: see [Model call recovery](../user/model-call-recovery.md). This page covers the settings, how failures are told apart, and diagnosis.

## Before you start

You need permission to change settings at the place you choose, and at least one provider whose models you are willing to fail over to if you want a switch. Test failure handling only with scripted model responses and synthetic recordings. Never provoke a real refusal, limit or capacity failure at a vendor to try a rule.

## 1. Choose the settings

| Key | Shipped value | Effect |
| --- | --- | --- |
| `calls.rules` | Busy and unreachable: retry three times, first delay 60 seconds, multiplier 2. Request refusal: continue three times. Session lost: continue once. Quota: wait for the reset. Anything else: wait for a person | The first matching rule owns an ordered chain of retry, continue, fail over and wait steps |
| `calls.repeat_limit` | 3 | How many automatic recoveries of one failure kind are allowed within the window; 0, shown as **Off**, stops automatic recovery |
| `calls.repeat_window` | 300 seconds | The rolling window of that guard, shared by the purposes of calls on the same subject |
| `calls.notify_on_stop` | false | When true, a guard stop sends the thread's owner one grouped Inbox notice |
| `calls.return` | `next_turn` | Go back to the thread's own choice at the next turn, when it is ready; `stay` keeps the candidate |

These are defaults, not caps: the editor and the API check the shape of a value, not its size.

1. Open the settings of the place and choose **Call recovery**.
2. Edit the rules. Put specific rules before general ones. An empty list turns rule handling off, so every failure waits for a person.
3. Save. Changing a rule checks pending work again before it acts.

A wait for a reset with no known vendor deadline needs a person. A reset or retry time that the vendor gives wins over the rule's own pause. A relative retry time stays anchored to the call that failed: skipping a candidate or restarting Orch does not restart the wait.

## 2. Match a failure

A rule can combine kind, reason, provider, model, purpose, HTTP status, words and a regular expression. Text matches ignore case.

All the agent programs and completion protocols share one classifier. Recognized structured codes and types from the vendor come first, then the HTTP status, then broad families of words. Advice such as "try again" does not outweigh a named cause. When word families conflict, the order is: missing access to a model or program, request refusal, full context, another unavailable model, quota, rejected credential, busy provider, unreachable provider, unknown failure.

- A filter's refusal of the request is `model_refused`, with reason `cyber`, `safety` or `other`.
- Access that was not granted to a model or program is `model_unavailable`, reason `access`. Retrying cannot create access: configure an eligible candidate or fix the provider's model entry.
- A bare 429 is busy. Evidence of quota, credit, billing or a usage limit marks a plan limit.
- An HTTP 401 or an explicit rejection of the credential can reject a shared credential. A bare 403 cannot.
- The loss of a confirmed running agent program session is `session_lost` with purpose `agent`. The shipped rule continues once and then waits for a person. If its machine is unavailable, the recovery waits for the machine to return without reserving another attempt, and **Stop retrying** still holds it. The lost process may have done work it did not report, so its conversation receives a visible **Continue**, even before any answer or tool has arrived. The original prompt is not replayed. An unconfirmed start still needs a person, and a person's **Stop** is never a recoverable loss.

Read the vendor's retained message before you change policy. A new wording can be handled by a scoped status, a words condition or a regular expression, without treating an unrelated provider as unavailable.

## 3. Choose candidates and account for attempts

- Every candidate names a configured provider and model, and can name a profile and supported parameters. Current access and the model's capabilities are checked before each use, and a skipped candidate keeps its reason. Parameters cannot replace the request's model, prompt, output limit or access checks. An embedding candidate must keep both the model and its dimensions.
- Retries and visible **Continue** inputs keep the recorded submission and approval boundaries of the input they come from. Revoking access stops derived work even if another session of the same person has wider access.
- Summaries and descriptions use the current access of the person who last wrote the setting.
- Usage and cost record the provider and model that served each paid attempt.
- An automatic switch uses the ordinary switch job, history plan, checks and rollback. Returning does not probe the vendor with a model request. If the original choice is structurally unavailable, change the provider's configuration or model information before the automatic return can consider it ready. An explicit change of the thread's choice dismisses stale recovery work.

## What a switch-off does

Turning a provider, model, profile or environment off is a cancellation, not a model failure. It reserves no failed-call attempt, runs no recovery rule and makes no automatic **Continue**. The resource's history names what was turned off and by whom, and usage already spent stays recorded. Summaries on a switched-off provider or model pause with that reason and no retry deadline. Turning it on again with **Run the waiting messages** resumes the stopped work and the waiting inputs; leaving that off holds the queues for **Continue** or **Resume**.

## Diagnose a wait or a stop

Start with the thread's live line, its attention banner and its recovery history. They show the failure, the rule, the action taken, the known wait and the attempt count. A repeated failure has one error line with its count. The history records the step limit with each attempt, so editing a rule later cannot change an old denominator.

- **Retry now** still enforces current limits and access. **Stop retrying** ends future attempts; it does not undo a running turn or finished tools.
- An accepted switch keeps its own **Cancel**. Stopping retries keeps its completion watcher, sends no follow-up input and keeps the return to the original choice for the next independent input.
- The guard is per subject and failure kind, across purposes, so a new purpose gives no fresh allowance. A guard stop leaves the partial conversation visible.
- For an automatic value (a summary, a title), read the setting's recovery reason, repair the policy, the access or the model choice, and ask for it again. A handoff summary that ran out of recovery follows the switch's own fallback to the plain transcript.

Recovery progress, deadlines and attempt reservations are durable database records. Do not edit them to clear a stop: use the product's controls and settings. A normal database backup restores them. An agent program may do its own retries or reviews that Orch does not see as separate calls; those follow that program's own controls. Recovery rules do not relax tool approval.

## If it does not work

| You see | What to do |
|---|---|
| A call fails with `model_unavailable` (access) and recovery retries in vain | Add an eligible candidate or fix the model entry on the provider; retrying cannot add access |
| A thread never leaves a switched-off candidate | `calls.return` is `stay`, or the original choice is still unavailable. Fix the provider or set `next_turn` |
| Recovery stopped after three tries | The guard reached `calls.repeat_limit` within `calls.repeat_window`. Fix the cause, or raise the limit, or set it to **Off** to stop automatic recovery altogether |
| Nobody learned of a stop | `calls.notify_on_stop` is false. Turn it on |
| A new vendor message is classified wrongly | Add a rule with a scoped status, words or regular expression above the general one |
