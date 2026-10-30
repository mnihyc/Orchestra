# Model call recovery

Recovery is place policy. Set it at system, organization, project or folder, or topic scope.
The ordinary inheritance and lock rules apply. A lower place cannot bypass an ancestor's lock.
The [user article](../user/model-call-recovery.md) explains the controls and visible states.

## Settings and defaults

| Key | Shipped value | Effect |
| --- | --- | --- |
| `calls.rules` | Busy and unreachable: retry three times, first delay 60 seconds, multiplier 2; request refusal: continue three times; quota: wait for reset; otherwise wait for a person | The first matching rule owns an ordered chain of retry, continue, failover and wait steps |
| `calls.repeat_limit` | `3` | Bounds automatic recoveries of one failure kind within the window; `0`, shown as Off, stops automatic recovery |
| `calls.repeat_window` | `300` seconds | The rolling window of the guard; shared by call purposes on the same subject |
| `calls.notify_on_stop` | `false` | A guard stop can send the thread owner one grouped Inbox notice |
| `calls.return` | `next_turn` | Return to the thread's own choice at the next turn when ready; `stay` keeps the candidate |

These numbers are defaults. The editor and API validate their shapes, without turning defaults into caps.
An empty rule list disables rule handling. A wait for reset without a known vendor deadline needs a person.
Vendor reset and retry times take priority over the rule's fallback pause.
A relative retry time stays anchored to its failed call; skipping a candidate or restarting Orch does not restart that wait.

## Match a failure

Rules may combine kind, reason, provider, model, purpose, HTTP status, contained words and a regular expression.
Text matches ignore case. Put specific rules first. Changing a rule rechecks pending work before it acts.

All supported adapters and completion protocols share one classifier. Recognized structured vendor codes and types
take priority, then HTTP status, then broad word families. Advice such as “try again” does not outweigh a named cause.
When word families conflict, the order is missing model or programme access, request refusal, full context,
other unavailable model, quota, rejected credential, busy provider, unreachable provider, then unknown failure.

A filter's refusal of this request is `model_refused`, with reason `cyber`, `safety` or `other`.
Access not granted to a model or programme is `model_unavailable` with reason `access`.
Retrying alone does not create missing access. Configure an eligible candidate or fix the provider's model entry.

A bare 429 is busy. Quota, credit, billing or usage-limit evidence identifies a plan limit.
An HTTP 401 or explicit credential rejection can reject a shared credential. A bare 403 cannot.
Inspect the retained vendor message before changing policy. A new wording can be handled by a scoped status,
contains or regular-expression condition without treating an unrelated provider as unavailable.

## Choose and account for attempts

Every failover candidate names a configured provider and model. It can also name a profile and supported parameters.
Current access and model capabilities are checked before each use. A skipped candidate keeps its reason.
Parameters cannot replace the request's model, prompt, output limit or access checks.
An embedding candidate must preserve both model identity and dimensions.

Retries and visible Continue inputs retain the origin input's recorded submission and approval boundaries.
Revocation stops derived work even when another session of the same person has broader access.
Summaries and descriptions use the current access of the person who last wrote the setting.
Usage and cost record the actual serving provider and model for each paid attempt.

Automatic switches use the ordinary switch job, history plan, checks and rollback.
Returning does not probe a vendor by sending a model request. A structurally unavailable original choice needs
changed provider configuration or model information before automatic return can consider it ready.
An explicit change to the thread's choice dismisses stale recovery work.

## Diagnose a wait or stop

Start with the thread's live line, attention banner and compact recovery history.
They show the failure, rule, actual action, known wait and attempt count. The switch job also shows a handoff
summary's recovery wait. Retry now still enforces current limits and access. Stop retrying ends future attempts;
it does not roll back a running turn or its completed tools.
An accepted switch keeps its own Cancel control. Stopping retries keeps its completion watcher,
sends no follow-up input, and retains the original-choice return policy for the next independent input.

The guard is per subject and failure kind, across purposes. A new purpose does not provide a fresh allowance.
A guard stop leaves the partial conversation visible. Notices are opt-in and use normal Inbox grouping.
For an automatic value, inspect the setting's recovery reason, repair policy/access/model choice, and request it again.
An exhausted handoff summary follows the switch's existing transcript fallback.

Recovery progress, deadlines and attempt reservations are durable database records. Do not edit them to clear a stop;
use the product's controls and settings. Restore them with the rest of the database in a normal backup.
An agent program may perform internal retries or reviews that it does not expose as separate Orch calls;
those remain governed by that program's own controls. Recovery rules do not relax tool approval.

## Deliberate switch-off

An administrative stop is cancellation, not a model failure. It does not reserve a failed-call attempt,
run a recovery rule or create an automatic Continue. The resource's history names what was switched off
and who switched it off. Usage already spent remains recorded.

Summaries on a switched-off provider or model pause with that reason and no retry deadline. Cancellation
does not enter the legacy retry timer. Enabling with **Run the waiting messages** resumes the stopped work
and waiting inputs; leaving it off holds the queues for Continue or Resume. A derived Continue retains
the original input's author and all recorded submission and approval boundaries, checked again when it acts.

Exercise failure handling only with scripted model responses and synthetic adapter recordings.
Never provoke a real vendor refusal, limit or capacity failure for a recovery check.

A repeated failure has one error line with its attempt count. Active recovery owns its controls;
older followed turns have no retry button. A stopped recovery shows its reason and actions only
in the attention banner. Automatic history records the step limit with the attempt, so later
rule edits cannot change its denominator. Older records without a limit show the attempt alone.
