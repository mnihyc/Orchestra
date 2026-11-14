/** What stands under a turn that is over: how it ended, what it ran with where that changed, the files it changed, and its acts.
 *
 * Its duration is on the line of its work and its tokens are under the pointer on its answer's toolbar. A turn that
 * failed says why, with Try again, and for an answer cut short, Continue; one that was stopped says by whom where no work
 * line does; a model or effort that changed stands in small quiet type; the changed files are one card with Review. A
 * turn with no answer carries its toolbar here. "Select this turn" picks every item of it at once.
 */
import { useMutation } from '@tanstack/react-query';
import { memo, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';

import { sendInput } from '@/api/threads';
import { hasParams, RanWith, effectiveTurnModel, effectiveTurnModelName } from '@/features/composer/RanWith';
import { effortDisplayName, modelDisplayName } from '@/features/models/names';
import { useTimelineContext } from '@/features/timeline/context';
import { TurnFiles, useTurnChanges } from '@/features/timeline/TurnFiles';
import { TurnActions, useTurnScope } from '@/features/timeline/TurnActions';
import { turnPasteSource } from '@/lib/turnPasteSource';
import { newActKey } from '@/lib/requests';
import { threadWriter, useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import type { Turn, TurnLimit } from '@/generated/api-types';
import type { TFunction } from 'i18next';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Tooltip } from '@/ui/Tooltip';
import { failureGroup, manualFailureActions } from './failures';
import { FailureAlert } from './FailureAlert';

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

function TurnFooterView({ turnId, inputItemId, answered = false, lined = false }: { turnId: string; inputItemId?: string | undefined; answered?: boolean; lined?: boolean }) {
  const { t } = useTranslation();
  const { threadId, canInteract, live, inspection } = useTimelineContext();
  const turn = useThreadPart(threadId, (view) => view.turns.get(turnId));
  const manual = useThreadPart(threadId, (view) => turn !== undefined && manualFailureActions(view, turn));
  const group = useThreadPart(threadId, useShallow((view) => turn === undefined ? { owner: turnId, attempts: 1 } : failureGroup(view, turn)));
  const previous = useThreadPart(threadId, (view) => {
    if (turn === undefined) return undefined;
    let before: Turn | undefined;
    for (const candidate of view.turns.values()) {
      if (candidate.ordinal < turn.ordinal && (before === undefined || candidate.ordinal > before.ordinal)) before = candidate;
    }
    return before;
  });
  const scope = useTurnScope(threadId, turnId);
  const changed = useTurnChanges(threadId, turnId);
  if (turn === undefined) return null;
  // What stands under a turn: how it ended where no work line says so, what it ran with where that changed, the files it changed, and its acts where it has no answer to carry them.
  const facts: ReactNode[] = [];
  if (turn.state === 'failed' && !lined) facts.push(<span key="failed">{t('timeline.footer.failed')}</span>);
  if (turn.state === 'interrupted' && !lined) {
    facts.push(<span key="interrupted">{turn.interrupted_by === null ? t('timeline.footer.interrupted') : t('timeline.footer.interruptedBy', { name: turn.interrupted_by.display_name })}</span>);
  }
  const cleanup = turn.command_cleanup ?? null;
  if (cleanup !== null) facts.push(<span key="cleanup">{t('running.cleanup', { ended: cleanup.ended, still: cleanup.still_running, unknown: cleanup.unknown })}</span>);
  const effective = effectiveTurnModel(turn), previousModel = previous === undefined ? undefined : effectiveTurnModel(previous);
  const rerouted = effective !== turn.model;
  const modelName = modelDisplayName(effectiveTurnModelName(turn), t);
  const model = turn.effort === null ? modelName : `${modelName} (${effortDisplayName(turn.effort, t)})`;
  if (rerouted || previous !== undefined && previousModel !== effective) facts.push(
    <Tooltip key="model" content={model}>
      <span>
        {model}
      </span>
    </Tooltip>,
  );
  if (!rerouted && previous !== undefined && previousModel === effective && previous.effort !== turn.effort) facts.push(<span key="effort">{turn.effort === null ? t('timeline.footer.effortDefault') : t('timeline.footer.effort', { effort: effortDisplayName(turn.effort, t) })}</span>);
  const paramsChanged = hasParams(turn.params) && (rerouted || previous === undefined || JSON.stringify(previous.params) !== JSON.stringify(turn.params));
  return (
    <div className="flex min-w-0 flex-col gap-2" data-footer={turnId}>
      <TurnFailure threadId={threadId} turn={turn} canInteract={canInteract} live={live} manual={manual} attempts={group.attempts} owner={group.owner} inputItemId={inputItemId} />
      {(facts.length > 0 || paramsChanged) && <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-text-subtle" data-turn-facts>
        {facts}
        {paramsChanged && <RanWith threadId={threadId} turn={turn} />}
      </div>}
      <TurnFiles turn={turn} changes={changed} />
      {(!answered || inspection !== undefined) && <TurnActions turn={turn} scope={scope} />}
    </div>
  );
}

export const TurnFooter = memo(TurnFooterView);
