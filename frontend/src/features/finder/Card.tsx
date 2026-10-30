/** The card of a person or a group found by name: who they are, how they are addressed, and where their page is, when there is one.
 *
 * A person's card reads their page through its normal privacy rules and the current direct shares between
 * the two people. The address is what other people type to find or to name them, so it is there to copy.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { ArrowLeft } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { userSharesQuery } from '@/api/profile';
import { openAccess } from '@/features/access/AccessDialog';
import { describe, KIND_ICONS, usernameOf } from '@/features/finder/destinations';
import { closeFinder } from '@/features/finder/store';
import { usePlaceLabel } from '@/features/navigation/placeLabels';
import { CardDetails } from '@/features/profile/CardDetails';
import type { NameHit } from '@/generated/api-types';
import { avatarText, identityColor } from '@/lib/grapheme';
import { Avatar } from '@/ui/Avatar';
import { Button } from '@/ui/Button';
import { CopyButton } from '@/ui/CopyButton';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

const DOCUMENT_KIND = 'document' as const;
const DOCUMENT_FOLDER_KIND = 'document_container' as const;

function Shares({ id }: { id: string }) {
  const { t } = useTranslation();
  const label = usePlaceLabel();
  const query = useInfiniteQuery(userSharesQuery(id));
  const items = query.data?.pages.flatMap((page) => page.items) ?? [];
  return <section aria-label={t('finder.card.shares')} className="flex flex-col gap-2 border-t border-border pt-3">
    <h3 className="text-sm font-medium">{t('finder.card.shares')}</h3>
    <p className="text-xs text-text-muted">{t('finder.card.sharesAbout')}</p>
    {query.isPending ? <SkeletonRows rows={2} /> : query.isError ? <ErrorState error={query.error} compact onRetry={() => void query.refetch()} /> : items.length === 0 ? <EmptyState compact title={t('finder.card.noShares')} /> : <ul className="flex flex-col gap-1">{items.map((item) => {
      const target = item.kind === 'node' ? {kind:item.node.kind,id:item.node.id,name:label(item.node) ?? item.node.name ?? ''}
        : item.kind === 'document_container' ? {kind:DOCUMENT_FOLDER_KIND,id:item.folder.id,name:item.folder.redacted ? t('ui.nameUnknown') : item.folder.name ?? t('ui.nameUnknown')}
        : {kind:DOCUMENT_KIND,id:item.document.id,name:item.document.title};
      return <li key={`${target.kind}:${target.id}`}>{item.kind === 'document_container'
        ? <Button variant="ghost" className="w-full justify-start" asChild><Link to="/document-folder/$folderId" params={{ folderId: item.folder.id }} onClick={closeFinder}>{target.name}</Link></Button>
        : <Button variant="ghost" className="w-full justify-start" onClick={() => { closeFinder(); setTimeout(() => openAccess(target,{share:false}),0); }}>{target.name}</Button>}</li>;
    })}</ul>}
    {query.hasNextPage && <Button variant="ghost" pending={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>{t('finder.card.moreShares')}</Button>}
  </section>;
}

export interface CardProps {
  hit: NameHit;
  onBack: () => void;
  /** Opens their page, when the application has one. */
  onPage: (() => void) | null;
}

export function Card({ hit, onBack, onPage }: CardProps) {
  const { t } = useTranslation();
  const username = usernameOf(hit);
  const Icon = KIND_ICONS[describe(hit)];
  return (
    <section aria-label={hit.name} className="flex flex-col gap-4 p-4">
      {hit.kind === 'user' && username !== null ? <CardDetails username={username} onNavigate={closeFinder} /> : <div className="flex items-center gap-3">
        <Avatar text={avatarText(hit.name)} color={identityColor(hit.id)} kind={hit.kind === 'group' ? 'org' : 'human'} size={40} />
        <div className="flex min-w-0 flex-col">
          <h2 className="truncate text-lg font-semibold text-text">{hit.name}</h2>
          <p className="flex items-center gap-1 text-sm text-text-muted">
            <Icon aria-hidden />
            {hit.kind === 'group' ? t('finder.card.group') : t('finder.card.person')}
            {username !== null && <span className="ml-2">@{username}</span>}
          </p>
        </div>
      </div>}
      {hit.address !== null && <CopyButton value={hit.address} label={t('finder.card.copyAddress')} />}
      {hit.kind === 'user' && <Shares id={hit.id} />}
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="ghost" icon={<ArrowLeft aria-hidden />} onClick={onBack}>
          {t('finder.card.back')}
        </Button>
        {onPage !== null && (hit.kind !== 'user' || username === null) && <Button onClick={onPage}>{t('finder.card.openPage')}</Button>}
      </div>
    </section>
  );
}
