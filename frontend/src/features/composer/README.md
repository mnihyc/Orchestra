# Inputs and delivery queue

Built: the box a person writes to the agent of a thread in. Enter queues and Shift+Enter starts a line (a
composition in progress is left alone); the split button is Send while idle and Stop while a turn runs,
with queue, steer and interrupt in its menu, and Esc twice stops the turn from the empty box. Alongside
it are + for uploads and references, the agent's binding, model and effort, the native approval mode and
the context ring that opens its breakdown. The phone row names the actual agent, the model and
compact spacing; the full current binding, effort and ring values stay in their tooltips, open pickers
and accessible names. Send and Stop use the same ordinary small control height as the model and mode
pickers, retain their intrinsic width and stand at the right end. A viewer keeps that same composer:
the box and attachment controls are disabled, the model and effort remain visible, and Send keeps its
disabled reason. `ViewerAccessLine.tsx` names the missing access and its native givers in one compact
row; Request access opens the existing request form with those permissions, model/effort options and
thread prefilled. Without a giver there is no request action. Archived threads keep their archive
line, and retained read-only inspections have no composer. Narrow-thread bottom tabs yield to the conversation.
What is written is kept per
thread, and on the server so that it is there on another device. Every waiting input, including the first
and those held by Stop, a cap or files, stands once in the queue above the composer. Its author edits its
words without losing attachments or quoted messages. Whoever drives the thread can change delivery,
send it now, remove it, or reorder it by drag, arrows or Alt+Up/Down. Send now steers a running turn;
unsupported steering offers explicit interruption. With no turn it moves the same input first. These
acts preserve existing holds. A move's answer is `{ input, inputs }`: the requested input and every
input the native act changed, including neighbors whose integer positions were renumbered. The queue
applies the stamped rows together through the same store as a live frame; it never ties a moved row
against a neighbor's stale position or asks for a reload to see the order. Duplicate or older answers
and frames cannot replace newer rows. A caller without View receives no neighbor contents from the
write's answer. A send that got no answer stays here and can be retried without being
made twice. A focused queue row or its action/drag control takes Alt+Up/Down for ordering; editors,
pickers, composition and Shift+Alt keep their own keys. Edit puts the caret at the end; Enter or
Ctrl/Cmd+Enter saves, Escape cancels, and Shift+Enter adds a line. Previews begin with meaningful message text, omitting the
scripted agent's delimiter. A pending steer keeps all its controls; during the native handoff the
same controls remain visible but disabled with the delivery reason, until its result removes the row.
Dragging starts from the row's handle with the same six-pixel intent threshold as the topic board. A
preview carries the picked message's first meaningful line, and a line inside the target row shows
whether it will land before or after that message. Only this bounded queue scrolls while carrying;
clipped rows outside its visible area cannot receive a drop. The source stays identified by its input
id when other messages arrive or the displayed row number changes. Release sends one position-only
update using the current predecessor, and the native answer changes the order; words, delivery and
holds stay intact. Escape, a drop outside, a missing row or a delivery that has already started makes
no update. The arrows and focused-row Alt+Up/Down remain the keyboard path, with translated drag
instructions and announcements for readers. Browser regression sources use real pointer presses and
travel, scroll the picked handle into view, await the indicated line and assert the exact moved input
and native request in both languages, both themes and desktop/phone widths.
A main-stream user message carrying that same input id also removes a stale queued or delivering row
from view immediately. The native input remains intact; a replay cannot show it a second time. While
a turn runs, the hint says Enter queues and Ctrl/Cmd+Enter steers, omitting unsupported steering.
A limit
in the way is said before a message is sent and may be let past by whoever may. Files are
uploaded through resumable sessions, kept as parts of the draft and delivered before the turn. Pending
files keep Send waiting, and an image this model cannot read offers Remove and Change model.
On returning to a thread, a known local draft keeps priority, including deliberately empty text and
removed attachments; only an uninitialized local field is restored from the server's saved draft.
A lane and its panel share one draft and debounce owner. Different writes are ordered behind an older
save, including the clear after sending; equal content is saved once. An accepted send preserves newer
words or files, and queued writes and acknowledgements cannot cross a signed-in account's lifetime.

The controls of what a thread runs with show their value alone, each opening a list of its own, and stand in one row under
the text box in this order: attach, Agent, Model, Effort, Fast (or Speed), Access, then Approvals, Plan, the context ring and
Send. They show the thread's own state and never what the provider allows (that is the provider's Models tab). The Model
list holds the models of every provider the person may use, in groups under the provider's name, so that choosing a model
chooses its provider, with the providers to ask for and the unavailable ones last; search stands above eight rows and the
list has its own scroll and a help entrance. A provider is no control of its own. The Effort list holds the efforts of the
chosen model with its default marked; exactly two speeds are one chip that flips and says the speed that is on; Access and
Context stand only where the model's managers switched them on, each saying where its value comes from, and the context
size is chosen from the ring's popover, with a field for another number, the adapter's sentence on where the conversation
is shortened and a line where a size crosses a step of the model's rate. A speed, an access program or a context size that
the managers switched on and the thread cannot use is still shown in its control, dimmed, and its list says why in a few
words (the agent offers none, the credential must stay with the provider, the environment or the provider is of the other
kind, the provider leaves it out, or a feature of agents is off) with, where the person may lift it, the act that does.
A change of model drops a choice the new one lacks and says so. For somebody who only drives the thread the values are plain
and read-only, with who can change them. `/model`, `/effort`, `/fast`, `/speed`, `/access`, `/context`, `/plan`, `/goal`,
`/side` (`/btw`), `/fork` and `/directory` in the box open the control or, written with what to choose, make the choice,
each only where the act exists, and the footer of a turn says what the turn ran with. The approval picker is shared with Parameters and offers only modes
declared by the harness and allowed by policy and confinement. It writes the displayed setting revision,
keeps a refused choice open with the current value, and never retries it silently. `/file`, `/document`
and @ open the reference picker; selecting a workspace file adds its real path, and selecting a document
attaches a copy through the ordinary permission-checked export and upload readers, ready for the next Send.
The row's small draft mark reports saving or a failed save without adding a status line. Disabled Send
and its chevron share one reason. Every list of the row is the same list (`PickList.tsx`): full-width single-line rows
with a check for the current one and no visible option border or selected fill, a muted second line only where there
is something to say, and groups under small headers shown only where they hold rows. Arrow keys, Home and End move
focus without changing a setting; Enter or Space selects it. The search field takes ArrowDown into the current row and
ArrowUp into the last row; filtered-out rows cannot retain the tab stop. A reader without permission to manage still
sees the choices read-only.

- `SettingConflict.tsx`: A refused setting choice names the reported changer and current model while leaving its controls open.
- `Composer.tsx`: IME-aware input to the agent and delivery controls; references added from files, differences and shells; and the limit prompt before a message goes past a limit.
- `Controls.tsx`: The next turn's controls beside its message: attachments, agent, model, effort, speed and access program, permission mode, context and delivery, and the line of what an archived thread ran with.
- `ApprovalMode.tsx`: The thread's native permission mode, chosen from its harness and place's offered modes with the current setting revision.
- `DeliveryControl.tsx`: One split action: Send when idle, Stop while a turn runs, with explicit queue, steer and interruption choices.
- `DeliveryMenu.tsx`: Queue, steer and interrupt for whoever may drive the thread; Side thread; broadcast choices.
- `PickList.tsx`: The one list the controls of the row share, and the chip that shows a value alone and opens it.
- `ModelEffort.tsx`: The model, its effort, its speed and its access program as separate controls; applies from the next turn.
- `ProviderModelChip.tsx`: The list of models in groups under their providers' names, with the providers to ask for and the unavailable ones last.
- `BindingLists.tsx`: The providers and agents of what a thread could be bound to, and the agent's list.
- `ThreadAgent.tsx`: The agent of a thread as a control of its own.
- `modelRows.tsx`: The rows of the lists of a model and of its effort.
- `Withheld.tsx`: A speed, an access program or a context size that the thread cannot use, shown off with why and what lifts it.
- `SpeedAndAccess.tsx`: Speed and access: the faster speeds and the access programs that a manager of the provider enabled for this model, and nothing that was only offered.
- `QueueTray.tsx`: One queue for every waiting input: author edits, delivery, Send now, removal and pointer or keyboard ordering.
- `queue.ts`: Waiting-input text edits, stable reorder targets and the delivery of an explicit Send now.
- `Attachments.tsx`: Upload, paste and file/image attachments.
- `DropTarget.tsx`: An accepting place names the destination only while files or an Orch reference are over it.
- `FileDrops.tsx`: Folder discovery is visible and cancellable before its files enter the shared uploader.
- `usePastedText.ts`: Folded paste drafts share one uploader and keep edits local until their replacement blob is ready.
- `Mentions.tsx`: Workspace files and readable documents added as references to the unsent message.
- `SlashMenu.tsx`: Conversation actions, a side thread and a fork among them.
- `Draft.tsx`: Per-user server-synchronized draft.
- `DraftMark.tsx`: A small saving or failure mark in the composer row, announced as status and explained on hover.
- `StopControl.tsx`: Explicit and keyboard interruption, for whoever may drive the thread.
- `ContextChoice.tsx`: The context of a thread: the sizes the model's managers opened, between the model's own window and the largest, with a field for another number, the sentence its adapter gives about where the conversation is shortened, and the line about a size that crosses a step of the model's rate.
- `modelState.ts`: What a thread runs with in speed, access and context, worked out from what its agent publishes, what its model's entry opens and what the thread itself chose; and what a change of model leaves of those choices.
- `runsWith.ts`: What a thread runs with, read and changed: the parameters of its agent with the thread's own choices among them, kept current as the thread's history says they changed, and the writes of a choice.
- `RanWith.tsx`: What a turn ran with beyond its model and effort, for the turn's footer: the speed, the access program and the context it was started with, as it recorded them.

The compact approval/sandbox/Plan row marks an inherited place default. Both native mode pickers use the same parameter/settings cache and one revision-guarded mode writer; sandbox choices come only from available_values. A current-session native divergence is named separately as the program last reported it, and a newer agreeing report retires that line. Nothing guesses the mode from the program name or an old segment. Ordinary sends and Send review share planningFor; an explicit Plan choice wins over the first-message default, whose unread state holds Send with its reason and a retry on failure.
