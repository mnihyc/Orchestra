/** A setting whose source is a thread opens that thread's exact existing setting editor. */
import { useViewLocation } from '@/features/workbench/pageContext';
import { useRouter } from '@tanstack/react-router';
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { SlidersHorizontal } from 'lucide-react';
import { nodeQuery } from '@/api/nodes';
import { SettingsEditor } from '@/features/settings/Editor';
import { Dialog } from '@/ui/Dialog';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { EmptyState } from '@/ui/EmptyState';

export function SettingSourceJumpHost() {
  const location = useViewLocation();
  const thread = /^\/thread\/([^/]+)\/?$/.exec(location.pathname)?.[1];
  const key = new URLSearchParams(location.searchStr).get('setting');
  return thread === undefined || key === null || key === '' ? null : <ThreadSource key={`${thread}:${key}`} id={thread} setting={key} />;
}

function ThreadSource({ id, setting }: { id: string; setting: string }) {
  const { t } = useTranslation();
  const router = useRouter();
  const node = useQuery({ ...nodeQuery(id), staleTime: 0, refetchOnMount: 'always' });
  const close = () => {
    const location = router.state.location;
    const search = new URLSearchParams(location.searchStr);
    search.delete('setting');
    router.history.replace(`${location.pathname}${search.size === 0 ? '' : `?${search.toString()}`}${location.hash === '' ? '' : `#${location.hash}`}`);
  };
  return <Dialog open onOpenChange={(open) => { if (!open) close(); }} title={t('helpRelease.settings')} size="lg">
    {node.isPending || node.isFetching ? <SkeletonRows rows={3} /> : node.isError ? <ErrorState compact error={node.error} onRetry={() => void node.refetch()} /> :
      node.data.kind !== 'thread' ? <EmptyState compact icon={<SlidersHorizontal aria-hidden />} title={t('helpRelease.sourceMissing')} /> : <SettingsEditor target={{ kind: 'node', id }} scope="thread" only={[setting]} place={{ name: node.data.name, owner: node.data.owner, access: node.data.access, kind: 'thread' }} />}
  </Dialog>;
}
