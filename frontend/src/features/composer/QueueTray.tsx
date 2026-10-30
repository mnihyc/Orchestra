/** One queue for every waiting input: author edits, delivery, Send now, removal and pointer or keyboard ordering.
 *
 * Every waiting input, including the first and the held ones, in
 * the order they will be delivered: who wrote each, its first line and how it is to be delivered. The
 * author alone edits the words; whoever may drive the thread takes an input back out, or puts the queue
 * in order. An input held for its author's allowance, or because the thread's owner may not use its
 * provider, says so and goes on waiting while the others go ahead. A message that was sent and not yet
 * answered stands here as "Sending…", and one that got no answer as "Not sent · Try again".
 */
import { DndContext, DragOverlay, PointerSensor, pointerWithin, useDraggable, useDroppable, useSensor, useSensors, type Active, type CollisionDetection, type DragEndEvent, type Over } from '@dnd-kit/core';
import { useMutation, useQuery } from '@tanstack/react-query';
import { ChevronDown, ChevronRight, GripVertical, LoaderCircle, Megaphone, Pencil, Send, Settings2, Target, X } from 'lucide-react';
import { useCallback, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { ApiError, client } from '@/api/client';
import { sourceDecisionOf } from '@/api/approvals';
import { DecisionStatus } from '@/features/approvals/Card';
import { UserChip } from '@/ui/UserChip';
import { cancelInput, loadNextThreadInputs, refreshThreadInputs, resumeQueue, retryInput, runInput, updateInput } from '@/api/threads';
import { TaskGuideLink } from '@/features/help/TaskGuideLink';
import { QUEUE_HELP } from '@/help/uploads';
import { Hint } from '@/features/help/Hint';
import { environmentQuery } from '@/api/environments';
import { TrustAnswers, TrustQuestion } from '@/features/resources/environments/HarnessTrust';
import type { AttachmentIssue, ContentPart, Delivery, ThreadInput, RequestDecisionStatus } from '@/generated/api-types';
import { composerWriter, discard, resend, useComposerStore, type SendingInput } from '@/features/composer/Draft';
import { modelName } from '@/features/composer/ModelEffort';
import { afterDropping, afterMoving, editableWords, nowDelivery, queuePreview, withEditedWords } from '@/features/composer/queue';
import { holds } from '@/lib/permissions';
import { isComposing } from '@/lib/shortcuts';
import { runningTurn, threadWriter, useThreadPart } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Select } from '@/ui/Select';
import { Textarea } from '@/ui/Textarea';
import { Tooltip } from '@/ui/Tooltip';
import { Pagination } from '@/ui/Pagination';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadReceipt } from '@/realtime/threadScope';

type InputChange = Parameters<typeof updateInput>[2];
const DELIVERIES: readonly Delivery[] = ['queue', 'steer', 'interrupt'];
// The same six-pixel intent threshold as the topic board: a press focuses; travel picks up.
const DRAG_STARTS_AFTER_PX = 6;
/** An input whose session command no longer checks out waits for its sender; each check that failed is said in words. */
const COMMAND_HOLDS = {
  'native_action.stale': 'composer.queue.heldCommandStale',
  'native_action.permission': 'composer.queue.heldCommandPermission',
  'native_action.arguments': 'composer.queue.heldCommandArguments',
  'native_action.unavailable': 'composer.queue.heldCommandUnavailable',
} as const;
const QUEUE_SCROLL = { canScroll: (element: Element) => element.hasAttribute('data-queue-list') };
interface QueueSlot { kind: 'queue-slot'; threadId: string; inputId: string; edge: 'before' | 'after' }
function queueSlot(value: unknown): QueueSlot | null {
  if (typeof value !== 'object' || value === null || !('kind' in value) || value.kind !== 'queue-slot'
    || !('threadId' in value) || typeof value.threadId !== 'string'
    || !('inputId' in value) || typeof value.inputId !== 'string'
    || !('edge' in value) || (value.edge !== 'before' && value.edge !== 'after')) return null;
  return { kind: 'queue-slot', threadId: value.threadId, inputId: value.inputId, edge: value.edge };
}

/** Clipped rows have bounds too; only a pointer inside this queue's visible list may choose one. */
const overQueue: CollisionDetection = (args) => {
  const list = args.droppableContainers[0]?.node.current?.closest('[data-queue-list]');
  const point = args.pointerCoordinates;
  if (list === null || list === undefined || point === null) return [];
  const bounds = list.getBoundingClientRect();
  if (point.x < bounds.left || point.x > bounds.right || point.y < bounds.top || point.y > bounds.bottom) return [];
  return pointerWithin(args);
};

/** A newly opened editor gets the caret once; typing never moves it back to the end. */
function focusEditor(editor: HTMLTextAreaElement | null) {
  if (editor === null) return;
  editor.focus();
  editor.setSelectionRange(editor.value.length, editor.value.length);
}

/** What an input carries without the files its issues name: a named part goes, and so do the named images and files of a forwarded part. */
export function withoutIssues(content: readonly ContentPart[], issues: readonly AttachmentIssue[]): ContentPart[] {
  const named = (part: number, collection: AttachmentIssue['collection'], index: number | null) =>
    issues.some((issue) => issue.part === part && issue.collection === collection && issue.index === index);
  return content.flatMap((part, at): ContentPart[] => {
    if (named(at, 'part', null)) return [];
    if (part.type !== 'forwarded') return [part];
    return [
      {
        ...part,
        images: part.images.filter((_, index) => !named(at, 'images', index)),
        attachments: part.attachments.filter((_, index) => !named(at, 'attachments', index)),
      },
    ];
  });
}

/** The files that hold an input, each with why, and what its author can do: try again, or send it without them. */
function HeldForFiles({ threadId, input, recovers }: { threadId: string; input: ThreadInput; recovers: boolean }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const take = (row: ThreadInput) => threadWriter.apply(read, { inputs: [row] });
  const retry = useMutation({ mutationFn: () => retryInput(read, input.id), onSuccess: take });
  const without = useMutation({
    mutationFn: () => updateInput(read, input.id, { content: withoutIssues(input.content, input.attachment_issues) }),
    onSuccess: (reply) => threadWriter.apply(read, { inputs: [...reply.inputs, reply.input] }),
  });
  const busy = retry.isPending || without.isPending;
  const words = withoutIssues(input.content, input.attachment_issues).length > 0;
  return (
    <div className="flex flex-col gap-1" data-held-files={input.id}>
      <ul className="flex flex-col gap-1 text-xs text-danger-text">
        {input.attachment_issues.map((issue) => (
          <li key={`${issue.part}-${issue.collection}-${issue.index ?? ''}`}>
            {t('composer.queue.fileIssue', { name: issue.name, why: issue.message })}
          </li>
        ))}
      </ul>
      {recovers ? (
        <span className="flex flex-wrap items-center gap-1">
          <Button size="sm" variant="secondary" pending={retry.isPending} disabled={busy} onClick={() => retry.mutate()}>
            {t('common.tryAgain')}
          </Button>
          {words && (
            <Button size="sm" variant="ghost" pending={without.isPending} disabled={busy} onClick={() => without.mutate()}>
              {t('composer.queue.sendWithout')}
            </Button>
          )}
        </span>
      ) : (
        <p className="text-xs text-text-muted">{t('composer.queue.heldFilesOthers', { name: input.author?.display_name ?? t('composer.queue.theSender') })}</p>
      )}
      {(retry.error ?? without.error) !== null && <ErrorAlert error={retry.error ?? without.error} />}
    </div>
  );
}

/**
 * A workspace's own harness configuration that waits for an answer: what was found and the two answers for whoever
 * manages the environment, the sentence of who decides for everybody else. The thread's messages wait meanwhile.
 */
export function WorkspaceTrustNotice({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const thread = useThreadPart(threadId, (view) => view.thread);
  const waits = thread?.state === 'blocked' && thread.blocked?.reason === 'workspace_trust';
  const environmentId = thread?.environment?.id ?? '';
  const environment = useQuery({ ...environmentQuery(environmentId), enabled: waits && environmentId !== '' });
  if (!waits) return null;
  const harnessKind = thread.binding?.agent.harness_kind ?? '';
  const harness = thread.binding?.agent.name ?? harnessKind;
  const asked = environment.data?.config_trust.find((entry) => entry.harness === harnessKind && entry.answer === null);
  const manages = environment.data?.access.permissions.includes('environment.manage') ?? false;
  if (asked === undefined || !manages) {
    return (
      <p className="text-xs text-text-muted" data-workspace-trust>
        {t('environments.configTrust.waiting', { environment: thread.environment?.name ?? '', harness })}
      </p>
    );
  }
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-warning px-2 py-2" data-workspace-trust>
      <p className="text-sm font-medium text-text">{t('environments.configTrust.asked', { harness })}</p>
      <TrustQuestion harness={harness} path={asked.path} />
      <TrustAnswers environmentId={environmentId} entry={asked} label={t('environments.configTrust.asked', { harness })} />
    </div>
  );
}

/** What a person's Stop holds of the queue, and Resume for whoever may drive the thread. */
function QueueHold({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const thread = useThreadPart(threadId, (view) => view.thread);
  const resume = useMutation({ mutationFn: (revision: number) => resumeQueue(read, revision) });
  const queue = thread?.queue;
  if (!queue?.held) return null;
  const may = holds(thread?.access, 'thread.interact');
  return (
    <div className="flex flex-wrap items-center gap-2 text-xs text-text-muted" data-queue-held>
      <span className="min-w-0 flex-1">
        {t('composer.queue.heldByStop', { name: queue.by?.display_name ?? t('composer.queue.someone'), count: queue.held_inputs })}
      </span>
      {may && (
        <Button size="sm" variant="secondary" pending={resume.isPending} onClick={() => resume.mutate(queue.revision)}>
          {t('composer.queue.resume')}
        </Button>
      )}
      {resume.error !== null && <ErrorAlert error={resume.error} />}
    </div>
  );
}


function QueueRow({ threadId, input, index, orderIndex, count, busy, fallback, change, move, dropAt, readOnly = false }: {
  threadId: string; input: ThreadInput; index: number; orderIndex: number; count: number; busy: boolean; fallback: boolean;
  change: (id: string, body: InputChange | null, done?: () => void) => void;
  move: (id: string, index: number) => void;
  dropAt: QueueSlot['edge'] | null;
  readOnly?: boolean;
}) {
  const { t } = useTranslation();
  const meId = useStore(client.session, (state) => state.userId);
  const thread = useThreadPart(threadId, (view) => view.thread);
  const [editing, setEditing] = useState(false);
  const [words, setWords] = useState('');
  const row = useRef<HTMLLIElement>(null);
  const readable = useThreadPart(threadId, (view) => view.closed === null && (view.read === null || view.read.ready));
  const may = readable && holds(thread?.access, 'thread.interact') && thread?.archived !== true;
  const mayExecute = may && !readOnly;
  const mine = may && input.author?.id === meId && input.source === 'user';
  const activeTurn = useThreadPart(threadId, runningTurn);
  const active = activeTurn !== null || thread?.state === 'running' || thread?.state === 'waiting_approval';
  const cannotSteer = useComposerStore((state) => state.cannotSteer[thread?.binding?.agent.id ?? ''] === true);
  const number = index + 1;
  const pending = input.state === 'pending';
  const locked = busy || !pending || !may;
  const editorLocked = locked || !mine;
  const { setNodeRef: setDragNode, setActivatorNodeRef, attributes, listeners, isDragging, active: carriedInput } =
    useDraggable({ id: input.id, disabled: !mayExecute || locked || editing });
  const { setNodeRef: setBeforeNode } = useDroppable({ id: `queue:${threadId}:${input.id}:before`, disabled: !mayExecute || locked,
    data: { kind: 'queue-slot', threadId, inputId: input.id, edge: 'before' } satisfies QueueSlot });
  const { setNodeRef: setAfterNode } = useDroppable({ id: `queue:${threadId}:${input.id}:after`, disabled: !mayExecute || locked,
    data: { kind: 'queue-slot', threadId, inputId: input.id, edge: 'after' } satisfies QueueSlot });
  const setRow = useCallback((node: HTMLLIElement | null) => { row.current = node; setDragNode(node); }, [setDragNode]);
  // A drag handle remains an ordinary button; it is not a pressed-state selector.
  const { role: _role, 'aria-pressed': _pressed, 'aria-roledescription': _description, ...dragAttributes } = attributes;
  const changedContent = withEditedWords(input.content, words);
  const held = input.queue_held
    ? t('composer.queue.heldStop')
    : input.state_detail === 'cap_reached'
      ? t('composer.queue.heldCap', { name: input.author?.display_name ?? t('composer.queue.theSender') })
      : input.state_detail === 'owner_lacks_access'
        ? t('composer.queue.heldOwner', { owner: thread?.owner.display_name ?? '', provider: thread?.binding?.provider.name ?? '' })
        : input.state_detail === 'second_step_required'
          ? t('composer.queue.heldSecondStep', { name: input.author?.display_name ?? t('composer.queue.theSender') })
          : input.state_detail === 'model_use_required'
            ? thread?.environment !== null && thread?.environment !== undefined
              ? t('composer.queue.heldUse', { model: modelName(thread), environment: thread.environment.name })
              : t('composer.queue.heldModel', { model: thread === null ? '' : modelName(thread) })
          : input.state_detail === 'files'
          ? t('composer.queue.preparing')
          : input.state_detail !== null && input.state_detail in COMMAND_HOLDS
          ? t(COMMAND_HOLDS[input.state_detail as keyof typeof COMMAND_HOLDS], { name: input.author?.display_name ?? t('composer.queue.theSender') })
          : null;
  // Somebody who may drive the thread runs a message that waits for the model on their own access; the server
  // asks for their own use of the model and answers with what is missing when they have none.
  const [decided,setDecided]=useState<{approvalId:string|null;status:RequestDecisionStatus}|null>(null);
  const decision=input.decision??(decided?.approvalId===(input.approval_id??null)?decided.status:null);
  const runnable = mayExecute && pending && input.state_detail === 'model_use_required' && input.author?.id !== meId && decision===null;
  const read = useThreadReadRef(threadId);
  const runner = useMutation({
    mutationFn: () => runInput(read, input.id),
    onSuccess: (reply) => threadWriter.apply(read, { inputs: [reply] }),
    onError:(error)=>{const current=sourceDecisionOf(error);if(current!==null)setDecided({approvalId:input.approval_id??null,status:current});},
  });
  const runsOn = input.run_as !== null && input.run_as.id !== input.author?.id ? input.run_as : null;
  // Recovery of an input is its author's; of one nobody sent, the thread's first owner's.
  const recovers = mayExecute && (input.author?.id ?? thread?.owner.id) === meId;
  const heldForFiles = input.state === 'pending' && input.state_detail === 'attachments';
  const Icon = input.source === 'announcement' ? Megaphone : input.source === 'goal' ? Target : Settings2;
  // What a goal's loop queued is told by the goal's own condition, and taking it out pauses the goal.
  const goal = thread?.goal ?? null;
  const preview = input.source === 'goal' && goal !== null ? t('goals.turn.queued', { condition: goal.condition }) : queuePreview(input.content);
  const sendDelivery = nowDelivery(active, cannotSteer || fallback);
  const interrupts = sendDelivery === 'interrupt';
  const sendReason = !pending ? t('composer.queue.deliveryStarted') : input.queue_held || input.state_detail !== null ? t('composer.queue.nowHeld') : interrupts ? t('composer.queue.nowInterrupt') : sendDelivery === 'steer' ? t('composer.queue.nowSteer') : t('composer.queue.nowFirst');
  const moveTo = (target: number) => { move(input.id, target); row.current?.focus(); };

  return (
    <li ref={setRow} tabIndex={may ? 0 : undefined} aria-label={t('composer.queue.row', { number })}
      aria-description={!pending ? t('composer.queue.deliveryStarted') : undefined}
      aria-keyshortcuts={mayExecute && pending ? 'Alt+ArrowUp Alt+ArrowDown' : undefined}
      onKeyDown={(event) => {
        // Row actions, including the drag handle, move their own row. Editors, pickers and
        // Shift+Alt keep their native or workbench keys.
        const target = event.target instanceof HTMLElement ? event.target : null;
        if (target === null || target.isContentEditable || target.closest('input,textarea,select,[role=combobox],[role=listbox]') !== null) return;
        if (!mayExecute || event.defaultPrevented || isComposing(event.nativeEvent) || !event.altKey || event.shiftKey || event.ctrlKey || event.metaKey) return;
        if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return;
        event.preventDefault();
        event.stopPropagation();
        if (pending && !busy && !editing && carriedInput === null) moveTo(orderIndex + (event.key === 'ArrowUp' ? -1 : 1));
      }}
      className="focus-inset relative flex flex-col gap-1 rounded-md border border-border bg-surface px-2 py-1 data-[queue-dragging=true]:border-accent-solid data-[queue-dragging=true]:bg-accent-subtle" data-queued={input.id} data-queue-dragging={isDragging && pending || undefined} aria-busy={busy || undefined}>
      <span ref={setBeforeNode} aria-hidden className="pointer-events-none absolute inset-x-0 top-0 h-1/2" />
      <span ref={setAfterNode} aria-hidden className="pointer-events-none absolute inset-x-0 bottom-0 h-1/2" />
      {dropAt !== null && mayExecute && !locked && <span aria-hidden data-queue-drop-line={dropAt}
        className={`pointer-events-none absolute inset-x-0 z-10 h-0.5 rounded-full bg-accent-solid forced-colors:bg-[Highlight] ${dropAt === 'before' ? 'top-0' : 'bottom-0'}`} />}
      <div className="flex min-w-0 items-center gap-2">
        {input.author !== null && input.source === 'user' ? (
          <span className="inline-flex min-w-0 max-w-1/2 shrink-0"><UserChip user={input.author} size={24} className="max-w-full" /></span>
        ) : (
          <Icon className="text-text-subtle" aria-label={input.source === 'announcement' ? t('composer.queue.announcement') : input.source === 'goal' ? t('goals.line.label') : t('composer.queue.system')} role="img" />
        )}
        {editing ? (
          <form
            className="flex min-w-0 flex-1 items-start gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              if (!editorLocked && changedContent.length > 0) change(input.id, { content: changedContent }, () => { setEditing(false); row.current?.focus(); });
            }}
          >
            <Textarea ref={focusEditor} label={t('composer.queue.editLabel')} hideLabel rows={2} value={words} onChange={(event) => setWords(event.target.value)} disabled={editorLocked} fieldClassName="min-w-0 flex-1" className="resize-none" aria-keyshortcuts="Enter Control+Enter Meta+Enter Escape"
              onKeyDown={(event) => {
                if (isComposing(event.nativeEvent)) return;
                if (event.key === 'Escape' && !busy) {
                  event.preventDefault(); event.stopPropagation(); setEditing(false); row.current?.focus(); return;
                }
                if (event.key !== 'Enter' || event.altKey || event.shiftKey) return;
                event.preventDefault();
                event.stopPropagation();
                event.currentTarget.form?.requestSubmit();
              }} />
            <Button type="submit" size="sm" variant="primary" pending={busy} disabled={editorLocked || changedContent.length === 0}>
              {t('composer.queue.save')}
            </Button>
            <Button size="sm" variant="ghost" disabled={busy} onClick={() => { setEditing(false); row.current?.focus(); }}>
              {t('common.cancel')}
            </Button>
          </form>
        ) : (
          <span data-queued-preview className="min-w-0 flex-1 truncate text-sm text-text" title={input.source === 'goal' ? `${preview}. ${t('goals.turn.queuedHint')}` : preview}>{preview || t('composer.queue.noPreview')}</span>
        )}
        {!pending && <span role="status" className="text-sm text-text-muted" title={t('composer.queue.deliveryStarted')}>{t(input.delivery === 'steer' ? 'composer.queue.steering' : 'composer.queue.sending')}</span>}
        {input.state === 'delivering' && input.delivery === 'steer' && mayExecute && <Tooltip content={input.state_detail === 'steering' ? t('composer.queue.steerTaken') : t('composer.queue.backToQueue')}><Button size="sm" variant="ghost" disabled={busy || input.state_detail === 'steering'} onClick={() => change(input.id, { delivery: 'queue' })}>{t('composer.queue.backToQueue')}</Button></Tooltip>}
      </div>
        {!editing && may && (
          <div className="flex flex-wrap items-center gap-1">
            {mayExecute && <><Select<Delivery> size="sm" label={t('composer.queue.deliveryLabel', { number })} hideLabel
              value={input.delivery} disabled={locked} fieldClassName="w-auto"
              options={DELIVERIES.map((delivery) => ({ value: delivery, name: t(`composer.queue.delivery.${delivery}`), description: delivery === 'steer' && active && cannotSteer ? t('composer.reason.noSteer') : t(`composer.queue.deliveryAbout.${delivery}`), disabled: delivery === 'steer' && active && cannotSteer }))}
              onValueChange={(delivery) => change(input.id, { delivery })} />
            <Tooltip content={sendReason}>
              <Button size="sm" icon={<Send aria-hidden />} disabled={locked} aria-label={t(interrupts ? 'composer.queue.interruptNowLabel' : 'composer.queue.sendNowLabel', { number })}
                onClick={() => change(input.id, active ? { delivery: sendDelivery, afterInputId: null } : { afterInputId: null })}>
                {interrupts ? t('composer.queue.interruptNow') : t('composer.queue.sendNow')}
              </Button>
            </Tooltip></>}
            <span className="ml-auto flex items-center gap-1">
            {mine && (
              <IconButton
                size="sm"
                label={t('composer.queue.editRow', { number })}
                icon={<Pencil aria-hidden />}
                disabled={locked}
                onClick={() => {
                  setWords(editableWords(input.content));
                  setEditing(true);
                }}
              />
            )}
                {mayExecute && <><IconButton ref={setActivatorNodeRef} size="sm" label={t('composer.queue.drag', { number })} icon={<GripVertical aria-hidden />} disabled={locked}
                  className="touch-none cursor-grab select-none active:cursor-grabbing" data-queue-handle={input.id} {...dragAttributes} {...listeners} />
                <IconButton
                  size="sm"
                  action="earlier"
                  label={t('composer.queue.moveRowUp', { number })}
                  disabled={locked || orderIndex === 0}
                  onClick={() => moveTo(orderIndex - 1)}
                />
                <IconButton
                  size="sm"
                  action="later"
                  label={t('composer.queue.moveRowDown', { number })}
                  disabled={locked || orderIndex >= count - 1}
                  onClick={() => moveTo(orderIndex + 1)}
                /></>}
                <IconButton size="sm" label={t('composer.queue.removeRow', { number })} icon={<X aria-hidden />} disabled={locked} onClick={() => change(input.id, null)} />
            </span>
          </div>
        )}
      {!mayExecute && <span className="text-xs text-text-muted">{t(`composer.queue.delivery.${input.delivery}`)}</span>}
      {held !== null && <div className="flex items-center gap-1"><p className="min-w-0 text-xs text-text-muted">{held}</p>{input.state_detail === 'model_use_required' && <Hint id="held-model" popover />}</div>}
      {decision!==null?<DecisionStatus source={decision}/>:runsOn !== null && <p className="flex items-center gap-1 text-sm text-text-muted">{t('agentIdentity.runsAs')}<UserChip user={runsOn} size={24}/></p>}
      {runnable && (
        <span>
          <Button size="sm" variant="secondary" pending={runner.isPending} disabled={busy} aria-label={t('composer.queue.runItLabel', { number })} onClick={() => runner.mutate()}>
            {t('composer.queue.runIt')}
          </Button>
        </span>
      )}
      {runner.error !== null && decision===null && <ErrorAlert error={runner.error} />}
      {heldForFiles && <HeldForFiles threadId={threadId} input={input} recovers={recovers} />}
    </li>
  );
}

function SendingRow({ threadId, entry, readOnly }: { threadId: string; entry: SendingInput; readOnly: boolean }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  return (
    <li className="flex items-center gap-2 rounded-md border border-dashed border-border-strong bg-surface px-2 py-1" data-sending={entry.clientId}>
      <span className="min-w-0 flex-1 truncate text-sm text-text-muted" title={queuePreview(entry.content)}>{queuePreview(entry.content) || t('composer.queue.noPreview')}</span>
      {entry.state === 'sending' ? (
        <span role="status" className="flex items-center gap-1 text-xs text-text-subtle">
          <LoaderCircle className="animate-spin" aria-hidden /> {t('composer.queue.sending')}
        </span>
      ) : (
        <span className="flex items-center gap-1">
          <span className="text-xs text-danger-text">{t('composer.queue.notSent')}</span>
          <Button size="sm" variant="secondary" disabled={readOnly} title={readOnly ? t('messageVersions.activeActions') : undefined} onClick={() => resend(read, entry)}>
            {t('common.tryAgain')}
          </Button>
          <Button size="sm" variant="ghost" onClick={() => discard(threadId, entry)}>
            {t('composer.queue.discard')}
          </Button>
        </span>
      )}
    </li>
  );
}

export function QueueTray({ threadId, queued, sending, readOnly = false }: { threadId: string; queued: readonly ThreadInput[]; sending: readonly SendingInput[]; readOnly?: boolean }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  // Open: what a person queued is what they look for. They can fold it away.
  const [open, setOpen] = useState(true);
  const [fallback, setFallback] = useState<string | null>(null);
  const [carried, setCarried] = useState<{ threadId: string; id: string; message: string } | null>(null);
  const [dropAt, setDropAt] = useState<QueueSlot | null>(null);
  const tray = useRef<HTMLElement>(null);
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: DRAG_STARTS_AFTER_PX } }));
  const thread = useThreadPart(threadId, (view) => view.thread);
  const page = useThreadPart(threadId, (view) => view.queuePage);
  const write = useMutation({
    mutationFn: async ({ id, body }: { id: string; body: InputChange | null }) => {
      if (body !== null) return updateInput(read, id, body);
      const input = await cancelInput(read, id);
      return { input, inputs: [input] };
    },
    onMutate: () => setFallback(null),
    onSuccess: (reply) => {
      threadWriter.apply(read, { inputs: [...reply.inputs, reply.input] });
      const captured = threadReceipt(reply.input);
      if (readOnly && captured !== undefined) {
        // The queue's own error state offers its existing retry if this bounded refresh fails.
        void refreshThreadInputs(captured).catch(() => undefined);
      }
    },
    onError: (error, { id, body }) => {
      if (body?.delivery !== 'steer' || !(error instanceof ApiError) || !error.is('capability.unsupported')) return;
      setFallback(id);
      const agentId = thread?.binding?.agent.id;
      if (agentId !== undefined) composerWriter.noteCannotSteer(agentId);
    },
  });
  const change = (id: string, body: InputChange | null, done?: () => void) => {
    if (write.isPending) return;
    write.mutate({ id, body }, { onSuccess: () => done?.() });
  };
  const total = queued.length + sending.length;
  const unsent = sending.some((entry) => entry.state === 'not_sent');
  if (total === 0 && page.nextAfter === null && !page.restartRequired) return null;
  // What was not sent is not hidden behind a folded tray.
  const shown = open || unsent;
  const ids = queued.filter((input) => input.state === 'pending').map((input) => input.id);
  const orderIndex = new Map(ids.map((id, index) => [id, index]));
  const move = (id: string, index: number) => {
    const after = afterMoving(ids, id, index);
    if (after !== undefined) change(id, { afterInputId: after });
  };
  const movable = !readOnly && holds(thread?.access, 'thread.interact') && !write.isPending;
  const carrying = movable && carried?.threadId === threadId && ids.includes(carried.id) ? carried : null;
  const indicated = carrying !== null && dropAt?.threadId === threadId
    && afterDropping(ids, carrying.id, dropAt.inputId, dropAt.edge) !== undefined ? dropAt : null;
  const destination = (active: Active, over: Over | null): QueueSlot | null => {
    const slot = queueSlot(over?.data.current);
    if (!shown || carrying?.id !== active.id || slot?.threadId !== threadId
      || afterDropping(ids, carrying.id, slot.inputId, slot.edge) === undefined) return null;
    return slot;
  };
  const finishDrag = ({ active, over }: DragEndEvent) => {
    const slot = destination(active, over);
    setCarried(null); setDropAt(null);
    if (slot === null || carrying === null) return;
    const after = afterDropping(ids, carrying.id, slot.inputId, slot.edge);
    if (after === undefined) return;
    change(carrying.id, { afterInputId: after });
    // Keep the same row focused while its acknowledged order changes, as the arrow controls do.
    tray.current?.querySelectorAll<HTMLLIElement>('[data-queued]').forEach((row) => {
      if (row.dataset.queued === carrying.id) row.focus();
    });
  };
  const message = (active: Active) => carried?.id === active.id ? carried.message
    : queuePreview(queued.find((input) => input.id === active.id)?.content ?? []) || t('composer.queue.noPreview');
  return (
    <DndContext sensors={sensors} collisionDetection={overQueue}
      autoScroll={QUEUE_SCROLL}
      onDragStart={({ active }) => {
        const input = queued.find((input) => input.id === active.id && input.state === 'pending');
        setCarried(movable && input !== undefined ? { threadId, id: input.id, message: queuePreview(input.content) || t('composer.queue.noPreview') } : null);
        setDropAt(null);
      }}
      onDragOver={({ active, over }) => setDropAt(destination(active, over))}
      onDragCancel={() => { setCarried(null); setDropAt(null); }} onDragEnd={finishDrag}
      accessibility={{ screenReaderInstructions: { draggable: t('composer.queue.dragInstructions') }, announcements: {
        onDragStart: ({ active }) => t('composer.queue.dragPicked', { message: message(active) }),
        onDragOver: ({ active, over }) => { const slot = destination(active, over); return slot === null ? t('composer.queue.dragNoPosition')
          : t(slot.edge === 'before' ? 'composer.queue.dragBefore' : 'composer.queue.dragAfter', { message: queuePreview(queued.find((input) => input.id === slot.inputId)?.content ?? []) || t('composer.queue.noPreview') }); },
        onDragEnd: ({ active, over }) => t(destination(active, over) === null ? 'composer.queue.dragCancelled' : 'composer.queue.dragDropped', { message: message(active) }),
        onDragCancel: () => t('composer.queue.dragCancelled'),
      } }}>
    <section ref={tray} aria-label={t('composer.queue.title', { count: total })} className="flex flex-col gap-1 rounded-lg border border-border bg-bg-subtle px-2 py-1" data-queue data-queue-dragging={carrying?.id}>
      <div className="flex items-center gap-1">
      <button
        type="button"
        aria-expanded={shown}
        onClick={() => setOpen((on) => !on)}
        className="flex min-w-0 flex-1 items-center gap-1 rounded-md text-left text-sm font-medium text-text-muted hover:bg-bg-muted hover:text-text"
      >
        {shown ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden />}
        {t('composer.queue.title', { count: total })}
      </button>
      <TaskGuideLink guide="message-queue" heading={QUEUE_HELP.heading} />
      </div>
      {!readOnly && <><QueueHold threadId={threadId} /><WorkspaceTrustNotice threadId={threadId} /></>}
      {shown && (
        <ul data-queue-list className="flex max-h-(--tray-max-height) flex-col gap-1 overflow-y-auto">
          {queued.map((input, index) => (
            <QueueRow key={input.id} threadId={threadId} input={input} index={index} orderIndex={orderIndex.get(input.id) ?? -1} count={ids.length} busy={write.isPending} fallback={fallback === input.id} change={change} move={move} readOnly={readOnly}
              dropAt={indicated?.inputId === input.id ? indicated.edge : null} />
          ))}
          {sending.map((entry) => (
            <SendingRow key={entry.clientId} threadId={threadId} entry={entry} readOnly={readOnly} />
          ))}
        </ul>
      )}
      {shown && (page.restartRequired ? <div className="flex flex-wrap items-center gap-2 text-xs text-text-muted">
        <span>{t('messageVersions.queueChanged')}</span>
        <Button size="sm" pending={page.loading} onClick={() => { void loadNextThreadInputs(read).catch(() => undefined); }}>{t('messageVersions.restartQueue')}</Button>
        {page.error !== null && <ErrorAlert error={page.error} />}
      </div> : <Pagination hasMore={page.nextAfter !== null} loading={page.loading} error={page.error}
        onMore={() => { void loadNextThreadInputs(read).catch(() => undefined); }} />)}
      {write.error !== null && <ErrorAlert error={write.error} />}
    </section>
    <DragOverlay dropAnimation={null}>{shown && carrying !== null && <div aria-hidden className="pointer-events-none max-w-(--container-form) truncate rounded-md border border-accent-solid bg-surface px-2 py-1 text-sm text-text shadow-md" data-queue-drag-preview>{carrying.message}</div>}</DragOverlay>
    </DndContext>
  );
}
