import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
/** "Fork" from a thread's menu, or "Fork from here" from a message: one request with nothing to choose, then the new thread opens by itself.
 *
 * The fork is made at once with the source's topic, environment, provider, profile and model and all of its
 * history; the job that makes it is followed until the new thread exists, and then it is opened, with no step
 * between. A fork that something stops says why; "Fork to…" is where a fork that changes something is planned.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import { forkNow } from '@/api/forks';
import { waitForJob } from '@/realtime/queryFeeds';
import { useActKey } from '@/lib/forms';
import { openChange } from '@/features/change/target';
import { toast } from '@/ui/Toast';
import { useErrorText } from '@/ui/errorText';
import type { AgentAccessChoice, Job } from '@/generated/api-types';

type ForkOutcome = { job: Pick<Job, 'state' | 'error'>; thread: string } | { accepted: string; thread: string };

export function useForkNow(threadId: string): { start: (toTurn?: number, agentAccess?: AgentAccessChoice) => void; pending: boolean } {
  const { t } = useTranslation();
  const failureText = useErrorText();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const key = useActKey();
  // The request and the job it starts are one act: it ends when the job does, and then the new thread opens, even
  // where the place it was asked from is gone by then.
  const start = useMutation({
    mutationFn: async ({ source, toTurn, agentAccess, reader, attempt }: { source: string; toTurn: number | undefined; agentAccess: AgentAccessChoice | undefined; reader: ReturnType<typeof libraryReader>; attempt: string }): Promise<ForkOutcome> => {
      if (!sameLibraryReader(reader)) throw new DOMException('The requesting reader changed.', 'AbortError');
      const reply = agentAccess === undefined ? await forkNow(source, attempt, toTurn) : await forkNow(source, attempt, toTurn, agentAccess);
      if (!sameLibraryReader(reader)) throw new DOMException('The requesting reader changed.', 'AbortError');
      if ('projection' in reply) return { accepted: reply.job_id, thread: reply.thread_id };
      try { return { job: await waitForJob(queryClient, reply.job), thread: reply.thread_id }; }
      catch { return { accepted: reply.job.id, thread: reply.thread_id }; }
    },
    onSuccess: (result, asked) => {
      if (!sameLibraryReader(asked.reader)) return;
      key.reset();
      if ('accepted' in result) {
        toast({ tone: 'info', title: t('duplicate.forkStarted') });
        openChange({ kind: 'thread', id: asked.source }, { kind: 'fork', job: result.accepted });
        return;
      }
      const { job, thread } = result;
      if (job.state === 'succeeded') void navigate({ to: '/thread/$threadId', params: { threadId: thread } });
      else toast({ tone: 'danger', title: t('change.forkNow.failed'), description: failureText(job.error).message });
    },
    onError: (error, asked) => { if (sameLibraryReader(asked.reader)) toast({ tone: 'danger', title: t('change.forkNow.failed'), description: failureText(error).message }); },
  });
  return { start: (toTurn?: number, agentAccess?: AgentAccessChoice) => {
    if (start.isPending) return;
    const reader = libraryReader();
    const intent = { source: threadId, toTurn, agentAccess, reader };
    start.mutate({ ...intent, attempt: key(intent) });
  }, pending: start.isPending };
}
