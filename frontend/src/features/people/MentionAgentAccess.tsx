/** A visible, unusable agent choice opens the existing exact missing-access request. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { nodeQuery } from '@/api/nodes';
import { AccessRequest } from '@/features/access/AccessRequest';
import { Dialog } from '@/ui/Dialog';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function MentionAgentAccess({ id, onClose }: { id: string; onClose: () => void }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const query = useQuery({ ...nodeQuery(id), staleTime:0 });
  const thread = query.data?.kind === 'thread' ? query.data : null;
  const refresh = () => { void query.refetch(); void client.invalidateQueries({queryKey:['chat-mentions']}); };
  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }} title={t('access.request.title')} size="sm">
    {query.isPending ? <SkeletonRows rows={3} /> : query.isError ? <ErrorAlert error={query.error} /> : thread === null ? null
      : <div className="flex flex-col gap-3"><p className="font-medium">{thread.name}</p>
        {thread.input_access.missing.length > 0 && <AccessRequest missing={thread.input_access.missing} objectId={id} forThread={id} fixed />}
        <Button size="sm" onClick={refresh}>{t('access.noAccess.tryAgain')}</Button>
      </div>}
  </Dialog>;
}
