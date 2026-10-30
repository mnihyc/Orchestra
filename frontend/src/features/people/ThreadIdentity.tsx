/** Current readable thread metadata distinguishes equal names wherever a thread is chosen. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { nodeQuery } from '@/api/nodes';
import { useFormatContext } from '@/lib/appearance';
import { ThreadStateChip } from '@/ui/ThreadState';

export function ThreadIdentity({ id }: { id: string }) {
  const { t } = useTranslation();
  const context = useFormatContext();
  const query = useQuery({ ...nodeQuery(id), staleTime: 0 });
  const thread = query.data?.kind === 'thread' && !query.isFetching && !query.isError ? query.data : null;
  if (thread === null) return null;
  return <span className="flex flex-wrap items-center gap-1 text-xs text-text-muted"><span>{thread.binding?.agent.name ?? t('access.agents.dialog.noAgent')}</span><ThreadStateChip display={thread.display} /><span>{new Date(thread.created_at).toLocaleString(context.locale, { timeZone: context.timeZone, month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })}</span></span>;
}
