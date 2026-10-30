/** Duration, the model by its display name and effort, recorded usage, and what the turn changed.
 *
 * The end of a turn, in small quiet type: how long it took, the tokens it used, the model by its
 * display name with its effort and, where it ran with them, its speed, its access program and its context,
 * and how many files it changed. A turn that failed or was stopped says
 * so (and who stopped it), and for whoever drives the thread offers to try it again. "Select this turn"
 * picks every item of it at once.
 */
import { useMutation } from '@tanstack/react-query';
import { ListChecks, RotateCcw, ScrollText } from 'lucide-react';
import { memo, useRef, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';

import { retryTurn, sendInput } from '@/api/threads';
import { ApiError } from '@/api/client';
import { hasParams, RanWith } from '@/features/composer/RanWith';
import { effortDisplayName, modelDisplayName } from '@/features/models/names';
import { RestoreFromTurn } from '@/features/source/Restore';
import { useTimelineContext } from '@/features/timeline/context';
import { turnActiveMs } from '@/features/timeline/turnTime';
import { openGiven } from '@/features/inspector/givenFocus';
import { TurnCost } from '@/features/usage/context/TurnCost';
import { useFormatContext } from '@/lib/appearance';
import { turnPasteSource } from '@/lib/turnPasteSource';
import { newActKey } from '@/lib/requests';
import { formatSpan } from '@/lib/time';
import { threadViewOf, threadWriter, useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import type { Turn, TurnLimit } from '@/generated/api-types';
import type { TFunction } from 'i18next';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { ErrorAlert } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { Tooltip } from '@/ui/Tooltip';
import { failureGroup, manualFailureActions } from './failures';
import { FailureAlert } from './FailureAlert';
import { queuedInputs } from './rows';

/** The existing Retry refusal when its original input is already in flight. */
const INPUT_ALREADY_WAITING = new Set([
  'The original input is waiting in the message queue or is being delivered',
  'The original input is already waiting or being delivered',
]);

/** What a limit that stopped a turn counts, in a sentence made from its structure, never from the vendor's words. */
function limitWords(limit: TurnLimit, t: TFunction): string {
  const hours = typeof limit.window_minutes === 'number' ? Math.round(limit.window_minutes / 60) : null;
  switch (limit.scope) {
    case 'model_family':
      return typeof limit.model_family === 'string' ? t('timeline.failure.limit.familyNamed', { family: limit.model_family }) : t('timeline.failure.limit.family');
    case 'spending':
      return t('timeline.failure.limit.spending');
    case 'rate':
      return typeof limit.retry_after_secs === 'number' ? t('timeline.failure.limit.rateWait', { count: limit.retry_after_secs }) : t('timeline.failure.limit.rate');
    case 'plan':
      return hours === null ? t('timeline.failure.limit.plan') : t('timeline.failure.limit.planWindow', { count: hours });
  }
}

/**
 * Why a turn failed, as it was recorded and as it reads after a reload: the sentence, what the limit counts and when
 * it ends, the harness's own words behind a disclosure, and for an answer cut short, Continue: a new message of the
 * person's into the same conversation, unlike Try again, which sends the turn's input once more.
 */
function TurnFailure({ threadId, turn, canInteract, live, manual, attempts, owner, inputItemId }: { inputItemId?:string; threadId: string; turn: Turn; canInteract: boolean; live: boolean; manual: boolean; attempts: number; owner: string }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const original = useThreadPart(threadId, (view) => turn.error?.code==='context_overflow'&&canInteract?turnPasteSource(view.inputs,view.items,turn.input_id,inputItemId):undefined);
  const sourceParts = original ?? [];
  const asFile = useMutation({
    mutationFn: () => sendInput(read, { content: sourceParts.map((part) => part.type === 'pasted_text' ? { ...part, delivery: 'file' as const, inline_text: undefined } : part), delivery: 'queue', clientId: newActKey() }),
    onSuccess: (reply) => threadWriter.apply(read, { inputs: [reply.input] }),
  });
  const proceed = useMutation({
    mutationFn: () => sendInput(read, { content: [{ type: 'text', text: t('timeline.failure.continueText') }], delivery: 'queue', clientId: newActKey() }),
    onSuccess: (reply) => threadWriter.apply(read, { inputs: [reply.input] }),
  });
  const error = turn.error;
  if (error === null || turn.state !== 'failed' || owner !== turn.id) return null;
  return (
    <div className="flex flex-col gap-1 text-sm text-text" data-turn-failure={turn.id}>
      <FailureAlert error={error} attempts={attempts} />
      {error.code === 'context_overflow' && canInteract && sourceParts.some((part) => part.type === 'pasted_text') && <Button size="sm" variant="secondary" pending={asFile.isPending} disabled={!live} onClick={() => asFile.mutate()}>{t('uploads.sendAsFile')}</Button>}
      {asFile.error !== null && <ErrorAlert error={asFile.error}/>}
      {typeof error.limit === 'object' && error.limit !== null && <p className="text-text-muted">{limitWords(error.limit, t)}</p>}
      {typeof error.resets_at === 'string' && (
        <p className="text-text-muted">
          {t('timeline.failure.resets')} <RelativeTime at={error.resets_at} />
        </p>
      )}
      {error.incomplete && manual && (
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-text-muted">{t('timeline.failure.incomplete')}</span>
          {canInteract && (
            <Button size="sm" variant="secondary" pending={proceed.isPending} disabled={!live} onClick={() => proceed.mutate()}>
              {t('timeline.failure.continue')}
            </Button>
          )}
        </div>
      )}
      {typeof error.said === 'string' && error.said.trim() !== '' && (
        <details className="text-xs text-text-muted">
          <summary className="cursor-default select-none">{t('timeline.failure.said')}</summary>
          <pre className="mt-1 font-mono whitespace-pre-wrap wrap-anywhere">{error.said}</pre>
        </details>
      )}
      {proceed.error !== null && <ErrorAlert error={proceed.error} />}
    </div>
  );
}

function TurnFooterView({ turnId,inputItemId }: { turnId: string;inputItemId?:string }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const { threadId, canInteract, live, inspection, selection, readOnly } = useTimelineContext();
  const read = useThreadReadRef(threadId);
  const turn = useThreadPart(threadId, (view) => view.turns.get(turnId));
  const manual = useThreadPart(threadId, (view) => turn !== undefined && manualFailureActions(view, turn));
  const originalInput = useThreadPart(threadId, (view) => turn === undefined ? undefined : view.inputs.get(turn.input_id));
  const originalWaiting = useThreadPart(threadId, (view) => turn === undefined || !manual ? undefined
    : queuedInputs(view).find((input) => input.id === turn.input_id)?.state);
  const group = useThreadPart(threadId, useShallow((view) => turn === undefined ? { owner: turnId, attempts: 1 } : failureGroup(view, turn)));
  const previous = useThreadPart(threadId, (view) => {
    if (turn === undefined) return undefined;
    let before: Turn | undefined;
    for (const candidate of view.turns.values()) {
      if (candidate.ordinal < turn.ordinal && (before === undefined || candidate.ordinal > before.ordinal)) before = candidate;
    }
    return before;
  });
  // "Select this turn": every item of it that can be picked, in the order of the conversation.
  const ofTurn = useThreadPart(
    threadId,
    useShallow((view) => view.order.filter((id) => view.items.get(id)?.turn_id === turnId)),
  );
  const records = useThreadPart(threadId, useShallow((view) => [...new Set(view.order.flatMap((id) => {
    const item = view.items.get(id);
    return item?.turn_id === turnId && item.kind === 'marker' && item.payload.marker === 'orch_context' ? [item.payload.data.model_input_id] : [];
  }))]));
  const changed = useThreadPart(
    threadId,
    useShallow((view) => {
      let files = 0;
      let added = 0;
      let removed = 0;
      for (const item of view.items.values()) {
        if (item.turn_id !== turnId || item.kind !== 'tool_call' || item.payload.tool.kind !== 'file_change') continue;
        files += item.payload.tool.changes.length;
        for (const change of item.payload.tool.changes) {
          added += change.added;
          removed += change.removed;
        }
      }
      return [files, added, removed] as const;
    }),
  );
  const refusalInputSeq = useRef<number | null>(null);
  const retry = useMutation({
    mutationFn: () => retryTurn(read, turnId, newActKey()),
    onSuccess: (reply) => threadWriter.apply(read, { inputs: [reply.input] }),
    onError: () => {
      const view = threadViewOf(read);
      const inputId = view.turns.get(turnId)?.input_id;
      refusalInputSeq.current = inputId === undefined ? null : view.inputs.get(inputId)?.seq_updated ?? null;
    },
  });
  const retryWaiting = retry.error instanceof ApiError && retry.error.is('request.invalid')
    && INPUT_ALREADY_WAITING.has(retry.error.problem.detail);
  // A delivered frame already held when the refusal arrived may be stale.
  const waitingRefusalSuperseded = retryWaiting && (originalWaiting !== undefined
    || (originalInput !== undefined && originalInput.state !== 'pending' && originalInput.state !== 'delivering'
      && (refusalInputSeq.current === null || originalInput.seq_updated > refusalInputSeq.current)));
  if (turn === undefined) return null;
  const took = turn.completed_at === null ? null : turnActiveMs(turn, Date.parse(turn.completed_at));
  const usage = turn.usage;
  const parts: ReactNode[] = [];
  if (turn.state === 'failed') parts.push(<span key="failed">{t('timeline.footer.failed')}</span>);
  if (turn.state === 'interrupted') {
    parts.push(<span key="interrupted">{turn.interrupted_by === null ? t('timeline.footer.interrupted') : t('timeline.footer.interruptedBy', { name: turn.interrupted_by.display_name })}</span>);
  }
  if (took !== null && took > 0) parts.push(<span key="took" title={t('timeline.footer.duration', { time: formatSpan(took, format) })}>{formatSpan(took, format)}</span>);
  const cleanup = turn.command_cleanup ?? null;
  if (cleanup !== null) parts.push(<span key="cleanup">{t('running.cleanup', { ended: cleanup.ended, still: cleanup.still_running, unknown: cleanup.unknown })}</span>);
  if (usage !== null && usage.total_tokens > 0) {
    // The figure is what was used, with a thin bar of what it was made of; the numbers are under the pointer
    // and the keyboard, and one click opens the turn's details.
    parts.push(<TurnCost key="usage" threadId={threadId} turn={turn} />);
  }
  const model = turn.effort === null ? modelDisplayName(turn.model_display,t) : `${modelDisplayName(turn.model_display,t)} (${effortDisplayName(turn.effort, t)})`;
  if (previous !== undefined && previous.model !== turn.model) parts.push(
    <Tooltip key="model" content={model}>
      <span>
        {model}
      </span>
    </Tooltip>,
  );
  if (previous?.model === turn.model && previous.effort !== turn.effort) parts.push(<span key="effort">{turn.effort === null ? t('timeline.footer.effortDefault') : t('timeline.footer.effort', { effort: effortDisplayName(turn.effort, t) })}</span>);
  const paramsChanged = hasParams(turn.params) && (previous === undefined || JSON.stringify(previous.params) !== JSON.stringify(turn.params));
  const [files, added, removed] = changed;
  if (files > 0) parts.push(<span key="changed">{t('timeline.footer.changed', { count: files, added, removed })}</span>);
  if (records.length > 0) parts.push(<IconButton key="context" size="sm" label={t('threadViews.given.context')} title={t('threadViews.given.received')} icon={<ScrollText aria-hidden />}
    onClick={() => {
      const record = records[0];
      if (records.length === 1 && record !== undefined) openGiven(threadId, { record });
      else openGiven(threadId, { turn: turnId });
    }} />);
  return (
    <>
    <TurnFailure threadId={threadId} turn={turn} canInteract={canInteract} live={live} manual={manual} attempts={group.attempts} owner={group.owner} inputItemId={inputItemId} />
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-text-subtle" data-footer={turnId}>
      {parts}
      {paramsChanged && <RanWith threadId={threadId} turn={turn} />}
      {(turn.state === 'failed' || turn.state === 'interrupted') && canInteract && manual && (
        originalWaiting === 'pending' || originalWaiting === 'delivering'
          ? <span>{t(originalWaiting === 'pending' ? 'timeline.footer.inputWaiting' : 'timeline.footer.inputDelivering')}</span>
          : <Button action="retryTurn" size="sm" variant="ghost" icon={<RotateCcw aria-hidden />} pending={retry.isPending} disabled={!live} onClick={() => retry.mutate()}>
            {t('timeline.footer.retry')}
          </Button>
      )}
      {inspection === undefined && readOnly !== true && ofTurn.length > 0 && (
        <IconButton action="selectTurn" size="sm" label={t('timeline.selection.turn')} icon={<ListChecks aria-hidden />} onClick={() => selection.pick(ofTurn)} />
      )}
      {inspection === undefined && turn.completed_at !== null && <RestoreFromTurn threadId={threadId} turn={turn} compact />}
      {retry.error !== null && !waitingRefusalSuperseded
        && <ErrorAlert error={retry.error} message={retryWaiting ? t('timeline.footer.inputWaitingOrDelivering') : undefined} />}
    </div>
    </>
  );
}

export const TurnFooter = memo(TurnFooterView);
