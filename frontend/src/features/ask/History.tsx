/** History stays personal across roots; each conversation continues to name its own governance. */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { askHistoryQuery } from '@/api/ask';
import { useAdminMode } from '@/features/access/AdminMode';
import { Checkbox } from '@/ui/Checkbox';
import { useState } from 'react';
import { orgsQuery } from '@/api/nodes';
import { KnownNodeChip } from '@/features/people/NamedChip';
import type { AskConversation } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorState } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';

export function AskHistory({ root, onChoose, onClose }: { root: string | null; onChoose: (conversation: AskConversation, inspection: boolean) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const mode = useAdminMode();
  const [inspection, setInspection] = useState(false);
  const inspecting = inspection && mode.data?.as_admin === true && root !== null;
  const history = useInfiniteQuery(askHistoryQuery(inspecting ? root : undefined, inspecting));
  const orgs = useQuery(orgsQuery).data ?? [];
  const groups = new Map<string, AskConversation[]>();
  for (const conversation of history.data?.pages.flatMap((page) => page.items) ?? []) groups.set(conversation.root_org_id, [...groups.get(conversation.root_org_id) ?? [], conversation]);
  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }} title={t('ask.history')}>
    {mode.data?.as_admin === true && root !== null && <Checkbox label={t('ask.inspectOrg')} checked={inspecting} onCheckedChange={setInspection} />}
    {history.isLoading ? <SkeletonRows rows={4} /> : history.isError ? <ErrorState error={history.error} onRetry={() => void history.refetch()} /> : groups.size === 0 ? <StateLine title={t('ask.noHistory')} /> :
      <div className="flex max-h-[60dvh] flex-col gap-4 overflow-y-auto">{[...groups].map(([root, conversations]) => {
        const org = orgs.find((org) => org.id === root);
        return <section key={root} className="flex min-w-0 flex-col gap-1">
          <div className="text-sm font-medium">{org === undefined ? t('ask.organization') : <KnownNodeChip node={{ kind: 'org', id: org.id, name: org.name, redacted: false }} />}</div>
          <ul>{conversations.map((conversation) => <li key={conversation.id}>
            <Button variant="ghost" className="w-full justify-start" onClick={() => onChoose(conversation, inspecting)}>
              <span className="min-w-0 flex-1 truncate text-left">{conversation.title || t('ask.newConversation')}</span><RelativeTime at={conversation.last_message_at} />
            </Button>
          </li>)}</ul>
        </section>;
      })}{history.hasNextPage && <Button size="sm" pending={history.isFetchingNextPage} onClick={() => void history.fetchNextPage()}>{t('ask.older')}</Button>}</div>}
  </Dialog>;
}
