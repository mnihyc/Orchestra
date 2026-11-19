# Changing where and on what work runs

An ordinary provider/profile switch uses one compact combined picker and one confirmation, with no step strip, generic explanatory
header, environment picker or Options panel. Once a new setup is chosen, its native model/effort remain adjustable in the same confirmation.
Preflight still decides readiness; real session-ending consequences, lossy-history warnings, access grants, a running-turn wait and failures
remain visible. Already-satisfied access and empty plan sections take no space. The same dialog shows the server job while it runs; it does
not insert another confirmation. Forks and environment changes keep their complete review.

Built: the one wizard that changes what a thread, an environment or a provider's threads run on, with the same four
views everywhere (the plan, the confirmation, the job's progress, the end) and the one entry point every other screen
calls, `openChange(target, { kind })`, whose host is drawn once in the shell. A plan is asked for from the server and
shown with ordinary shared selects, three short lists under them (what changes, what stays as it is, what
ends or is removed) and what stops it, each blocker with what would lift it and, where access is missing, whom to ask.
Confirming locks what is changed and starts a job that is the server's: it is followed from any window, hidden and
opened again from the banner of the thread it holds, cancelled and undone until its commit, and ends in Done or in
where it failed and what was undone. A plan is good for fifteen minutes and is made again when it is not.

A short, visible, ARIA-associated reason stays beside a disabled Switch/Fork: no change, missing model, checking, failed read,
expired plan or the native blocker. Cancel and the act remain ordinary adjacent controls at phone widths. A model reported on both
providers is explicitly frozen into the new request, so that displaying the current model cannot leave the wizard unsettled forever.
The debounce gates the request itself, not merely its key; resetting a choice, a placeholder, an expired plan or a failed refresh cannot
confirm an older plan. One logical prompt scope encloses the presenter's reads: its loading-to-loaded replacement keeps its original
opening order, while a newer prompt pauses the covered form's effects and retains its choices. Component `switch-readiness`/`change-labels`
and browser `start-help-switch` are the unrun regression sources; `help-layers` covers the delayed-presenter ordering.

- A switch of a blocked or running thread to another provider, agent or environment: the provider that can take it
  is chosen for a thread held back by its provider, a model and an effort are chosen again with it, who may go on
  talking to it and the step that opens the new provider to them are shown, and what is left behind is said.
- A fork of a thread: all of its history, up to a message (from the menu of any item), a range of turns, a summary
  or none; into the same environment, another one or a new one at a place, with all, some (by patterns) or none of
  the files; whose page the new thread has, its name, the agent's notes. A side thread is kept as a thread the same way.
- An environment brought up to date or made again, or moved to another host, from its page; what is idle at a host or
  a place stopped to make room, listed exactly as it will stop (its threshold and the processes asked again at each change).
- Several threads switched at once, from a topic, a provider, an environment or a selection: one plan each, the ones
  that cannot go named with why, the threads the reader cannot see counted, one bar for the batch and a row for each
  thread, the batch watched and cancelled.

Forks, switches, side threads and batches use server plans and jobs. Environment update/recreate/move requests use
the native transition routes, with destinations decided by declared lifecycle and current machine policy. The
files of a fork are chosen by patterns. The checks of a job are shown
only where the job prints them. Provider updates and selected environment/provider batches open the shared fleet update dialog.

- `target.ts`: What can be changed and how, and the one entry point every screen calls: `openChange`.
- `ChangeHost.tsx`: Draws the change that was asked for with `openChange`: the one place in the frame of the application where the wizard stands.
- `Frame.tsx`: The dialog every change stands in: its title, the four steps with the one it is at, and the acts at its foot.
- `ThreadChange.tsx`: A fork or a switch of one thread: ordinary shared pickers, the plan, confirmation, progress and result.
- `BatchChange.tsx`: Several threads switched at once, from a topic, a provider, an environment or a selection: one plan for each, and one bar for the batch.
- `EnvironmentChange.tsx`: An environment brought up to date, made again or moved to another host: what changes, what stays, what ends, and the threads it holds meanwhile.
- `Recycle.tsx`: Adjustable threshold and exact dry-run consequences: what is idle at a host or a place, listed before it is stopped.
- `Sentence.tsx`: The first-control focus helper used by the pickers; the older sentence/editor utilities are retained for existing consumers, not rendered by the wizard.
- `modelLabel.ts`: The reported model display name and catalogue effort label, keeping routing values in requests.
- `choices.ts`: What a person has chosen in the wizard of a fork or a switch, and the request that asks for exactly that.
- `plan.ts`: What a plan says, in words: the three short lists (what changes, what stays, what ends), how the history travels, how long it takes.
- `Lists.tsx`: What changes, what stays as it is, and what ends or is removed: three short lists, always in view, so that nobody has to work out what a plan means.
- `Blockers.tsx`: What stops a plan, each with what would lift it; one that is missing access names what is missing and whom to ask.
- `Depends.tsx`: Who of the people on the thread's page may go on talking to it on the new side, and the step that opens the new provider to them.
- `RunsOn.tsx`: Provider, profile, model and effort the result runs on.
- `Where.tsx`: Where the result works: the same environment, another one the person manages or a new one at a place, with the files that go along into it.
- `Files.tsx`: Which files go along into another environment: all, none, or those that match what is written; the home directory and what was written in private views; the agent's notes.
- `History.tsx`: How much history a fork takes: all, up to any turn (which is what forking from a message asks), a range, a summary, none; and how it travels.
- `summary.ts`: Whether this conversation's inherited configuration chooses a summary producer.
- `Options.tsx`: The parameters of a plan with the values they start from: a running turn, the wait, the checks, keeping the old side.
- `Time.tsx`: The time a change takes, and what a turn that is running means for it.
- `Progress.tsx`: The bar with its percentage and phase, the checks one by one, and cancellation; the same from any device.
- `Measurements.tsx`: Reusable background counts on the ordinary job feeds, live sizes and cancellation, and one replan when counts finish.
- `TargetBuilds.tsx`: The immutable compatibility result and warnings for every target program build.
- `UpdateNotify.tsx`: Optional update notices with the shared bounded list of affected people.
- `Result.tsx`: Done: what was checked, what was left out and what is being removed. Or where it stopped, that everything was undone, and Try again.
- `refresh.ts`: Asks again for what a job that is over may have changed: where environments are, what runs in them, which threads use which provider.

Provider, agent, model, effort, environment and fork history use ordinary
shared selects, without boxed editors or repeated instructions. Short descriptive names say whether files are shared
or copied. Rich destination facts and refusals stay inside the open dropdown; native permissions and values still
decide what can be chosen. Only a selected access/removal consequence opts into the shared quiet sentence. Help has one
home in the rail. The result has one outcome, and reset actions name their field. Model display names feed the picker,
before/after plan and completion text through one helper; the model routing name stays in the request. A closed plan
does not fetch its choices, and the optional arrangement view mounts only when opened.

Regression sources: component `change-compact`, `change-labels`, `change-controls`, `move-machine-choice`, `start-actions`
and the retained `change` cases cover English/Chinese labels, selection and policy gates, unchanged routing payloads,
one selected consequence, duplicates and grouped creation actions. Unit `change-model-labels` covers plan/result names
and truthful unnamed values. Browser `change-controls` covers ordinary pickers/options/confirmation/progress/results,
environment transitions and retry, and the loading new-thread action row at 1440/400 in light/dark. The geometry helper
resolves the actual height token and intrinsic action widths, allowing one CSS pixel only for fractional rounding.
Legacy sentence/editor test cases are not proof of the current picker UI. The integrator runs all gates and walks the
shipping build in both languages/themes; writers run no builds or tests.
