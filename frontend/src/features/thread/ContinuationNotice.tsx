/** Explicit history browsing and native preparation status, beside this thread's composer. */
import { useMutation } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { continuationOutcomeUnknown, activateContinuation } from '@/api/continuations';
import { selectThreadContinuation } from '@/api/threadRead';
import { PromptNavigation, type SelectPromptVersion } from '@/features/timeline/PromptNavigation';
import type { ContinuationActiveTurn, ThreadContinuationActivateBody } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { newActKey } from '@/lib/requests';
import { type ThreadView } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Meter } from '@/ui/Meter';

export function ContinuationNotice({ threadId, view, onSelect }: {
  threadId: string; view: ThreadView; onSelect?: SelectPromptVersion | undefined;
}) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const [accepted, setAccepted] = useState<{ id: string; jobId: string; seq: number } | null>(null);
  const projection = view.read?.projection;
  const prompt = view.viewedPrompt;
  const target = projection?.viewed_continuation_id;
  const select = (id: string | null) => onSelect === undefined ? selectThreadContinuation(read, id) : onSelect(id);
  const activate = useMutation({
    mutationFn: ({ id, body }: { id: string; body: ThreadContinuationActivateBody }) => activateContinuation(read, id, body),
    onSuccess: (reply) => setAccepted({ id: reply.continuation_id, jobId: reply.job.id, seq: reply.seq }),
  });
  useEffect(() => {
    if (accepted !== null && target === accepted.id && projection?.active_continuation_id === accepted.id && prompt?.status !== 'preparing') {
      if (onSelect === undefined) selectThreadContinuation(read, null); else onSelect(null);
    }
  }, [accepted, target, projection?.active_continuation_id, prompt?.status, onSelect, read]);
  if (view.read?.selection === null || view.read === null) return null;
  const status = prompt?.status;
  const mayContinue = projection !== null && projection !== undefined && target !== projection.active_continuation_id
    && view.read.ready && (prompt === null || status === 'ready' || status === 'failed') && view.thread !== null && !view.thread.archived && holds(view.thread.access, 'thread.interact')
    && view.thread.input_access.allowed;
  const turn = view.thread?.current_turn_id ?? null;
  const retryOnly = activate.variables?.id === target && activate.variables !== undefined && continuationOutcomeUnknown(activate.error);
  const jobRunning = prompt?.job?.state === 'queued' || prompt?.job?.state === 'running';
  const jobFailed = prompt?.job?.state === 'failed' || prompt?.job?.state === 'cancelled';
  const busy = activate.isPending || status === 'preparing' || jobRunning
    || accepted !== null && accepted.id === target && prompt?.job?.id !== accepted.jobId
      && (view.viewedPromptSeq === null || view.viewedPromptSeq < accepted.seq) && projection?.active_continuation_id !== target;
  const continueHere = (action: ContinuationActiveTurn['action']) => {
    if (target === undefined || projection === null || projection === undefined || view.thread === null || busy) return;
    const body = retryOnly ? activate.variables.body : {
      client_id: newActKey(), expected_active_continuation_id: projection.active_continuation_id,
      expected_execution_revision: projection.execution_revision, expected_queue_revision: view.thread.queue.revision,
      ...(turn === null ? {} : { active_turn: { turn_id: turn, action } }),
    };
    activate.mutate({ id: target, body });
  };
  const hasPrompt = prompt !== null && [...view.items.values()].some((item) => item.kind === 'user_message' && item.payload.input_id === prompt.input_id);
  return <div className="flex flex-col gap-2 rounded-lg border border-border bg-bg-subtle px-3 py-2" data-continuation-notice>
    <div className="flex flex-wrap items-center gap-2">
      <p role="status" className="min-w-0 flex-auto text-sm text-text-muted">
        {t(status === 'preparing' || busy ? 'messageVersions.preparing' : status === 'failed' || jobFailed ? 'messageVersions.failed' : 'messageVersions.browsing')}
      </p>
      {!hasPrompt && prompt !== null && <PromptNavigation threadId={threadId} prompt={prompt} onSelect={onSelect} />}
      <Button size="sm" variant="ghost" onClick={() => select(null)}>{t('messageVersions.backToActive')}</Button>
    </div>
    {prompt !== null && prompt.job !== null && (status === 'preparing' || jobRunning) && <Meter look="progress" value={prompt.job.percent} label={t('messageVersions.preparing')} />}
    {mayContinue && <div className="flex flex-wrap items-center gap-2">
      <Button size="sm" pending={busy} onClick={() => continueHere('wait')}>
        {t(retryOnly || turn === null && (status === 'failed' || jobFailed) ? 'common.tryAgain' : turn === null ? 'messageVersions.continueHere' : 'messageVersions.waitContinue')}
      </Button>
      {turn !== null && !retryOnly && <Button size="sm" disabled={busy} onClick={() => continueHere('interrupt')}>{t('messageVersions.interruptContinue')}</Button>}
      <span className="text-xs text-text-muted">{t('messageVersions.filesUnchanged')}</span>
    </div>}
    {activate.error !== null && <ErrorAlert error={activate.error} />}
  </div>;
}
