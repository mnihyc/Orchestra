---
group: work
screens: [/thread/$threadId]
keywords: [agent schedules, scheduled inputs, wake-up, recurring, cron, cancel, schedule]
summary: Let an agent wake its thread on a schedule you can see, pause and end.
---

# Let the agent schedule future work

An agent can arrange a later input on its own thread, such as checking a CI run in ten minutes or repeating a short review. Orch keeps these schedules visible and cancellable. They use the same input queue as people's schedules.

## Enable agent schedules

Open the thread's Schedules view. Agent schedules is Off by default. Turn it On there, or set the inherited choice in system, organization, project or topic settings. The usual setting locks apply.

Agent schedule duration and Agent schedule runs supply the default end for recurring work: 24 hours and 20 runs. These are defaults; an explicit end or positive run count can differ. Changing the defaults does not rewrite an existing schedule's end.

## Read the pending work

Each pending agent schedule has a line after the conversation, with its next time, note and Cancel. Schedules lists people's and agent entries together: who the agent acts for, recurrence, end, next run and the latest outcome. A paused schedule has no promise to continue at a time. Failed, held or skipped runs keep their actual outcome.

For example, ask the enabled agent to check the CI run in ten minutes. Its pending line appears when the schedule is accepted. The later input is an ordinary message marked Woken by its schedule, after its actual run is matched. A message that merely mentions a schedule is not that marker. Older visible runs are checked through their native history.

## Cancel or finish

Anyone with current thread control can cancel future occurrences with one click. Cancellation leaves an input that has already entered the ordinary queue there; use the queue's own controls for it. Turning the scheduling tool off prevents new calls and does not stand in for cancelling an existing schedule.

Each run checks the starter's current permissions and the provider's allowance again. A schedule reserves neither rights nor credits. No notification is sent by default. The managed schedule belongs to Orch; the agent program's private timers remain unavailable. See [running thread views](running-thread-views.md) for the queue, current work and background tasks.
