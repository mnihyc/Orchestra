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
import { newActKey } from '@/lib/requests';
import { toast } from '@/ui/Toast';
import { useErrorText } from '@/ui/errorText';

export function useForkNow(threadId: string): { start: (toTurn?: number) => void; pending: boolean } {
  const { t } = useTranslation();
  const failureText = useErrorText();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  // The request and the job it starts are one act: it ends when the job does, and then the new thread opens, even
  // where the place it was asked from is gone by then.
  const start = useMutation({
    mutationFn: async (toTurn: number | undefined) => {
      const reply = await forkNow(threadId, newActKey(), toTurn);
      return { job: await waitForJob(queryClient, reply.job), thread: reply.thread_id };
    },
    onSuccess: ({ job, thread }) => {
      if (job.state === 'succeeded') void navigate({ to: '/thread/$threadId', params: { threadId: thread } });
      else toast({ tone: 'danger', title: t('change.forkNow.failed'), description: failureText(job.error).message });
    },
    onError: (error) => toast({ tone: 'danger', title: t('change.forkNow.failed'), description: failureText(error).message }),
  });
  return { start: (toTurn?: number) => start.mutate(toTurn), pending: start.isPending };
}
