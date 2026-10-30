/** Reading an adopted session's missing earlier history again, for whoever manages the thread. */
import { useMutation } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { retryAdoptedHistory } from '@/api/adoption';
import { useThreadRow } from '@/realtime/stores/thread';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';

export function RetryAdoptedHistory({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const thread = useThreadRow(threadId);
  const retry = useMutation({ mutationFn: () => retryAdoptedHistory(threadId, crypto.randomUUID()) });
  if (!holds(thread?.access, 'thread.manage')) return null;
  return <div className="flex flex-col items-center gap-1">
    <Button size="sm" variant="ghost" pending={retry.isPending} onClick={() => retry.mutate()}>{t('adopt.history.readAgain')}</Button>
    {retry.error !== null && <ErrorAlert error={retry.error} />}
  </div>;
}
