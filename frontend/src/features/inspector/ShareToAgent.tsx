/** Choose what to give before sharing it with this thread's agent. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { pageQuery, type AccessTarget } from '@/api/access';
import { ShareWithAgentsDialog } from '@/features/access/ShareWithAgents';
import { NamePicker } from '@/features/search/NamePicker';
import type { NameKind, Thread } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonRows } from '@/ui/Skeleton';

const SHARE_KINDS: readonly NameKind[] = ['org', 'project', 'topic', 'thread', 'document', 'chat'];

export function ShareToAgent({ thread, onClose }: { thread: Thread; onClose: () => void }) {
  const { t } = useTranslation();
  const [source, setSource] = useState<AccessTarget | null>(null);
  const page = useQuery({ ...pageQuery(source ?? { kind: 'thread', id: '' }), enabled: source !== null, retry: false });
  if (source !== null && page.data !== undefined && page.data.gives.agents.length > 0) return <ShareWithAgentsDialog target={source} page={page.data} initialRecipient={{ id: thread.id, name: thread.name }} open onClose={onClose} />;
  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }} title={t('threadViews.knowledge.share')} footer={<Button onClick={onClose}>{t('common.cancel')}</Button>}>
    <NamePicker label={t('threadViews.knowledge.choose')} kinds={SHARE_KINDS} chosen={source?.name ?? null} anything={t('threadViews.knowledge.chooseAny')} placeholder={t('threadViews.knowledge.choosePlaceholder')} nothing={t('threadViews.knowledge.chooseNothing')} onChoose={(hit) => {
      if (hit === null) { setSource(null); return; }
      if (hit.id !== thread.id && (hit.kind === 'org' || hit.kind === 'project' || hit.kind === 'topic' || hit.kind === 'thread' || hit.kind === 'document' || hit.kind === 'chat')) setSource({ kind: hit.kind, id: hit.id, name: hit.name });
    }} />
    {source !== null && page.isPending && <SkeletonRows rows={3} />}
    {page.error !== null && <ErrorState compact error={page.error} onRetry={() => void page.refetch()} />}
    {source !== null && page.data?.gives.agents.length === 0 && <InlineAlert tone="info">{t('threadViews.knowledge.cannotShare')}</InlineAlert>}
  </Dialog>;
}
