/** Daily work first on an org home: named personal waiting threads and recent visits with one readable reason each. */
import { useQuery } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { namesQuery, NAME_LIMIT_MAX } from '@/api/search';
import { openFinder } from '@/features/finder/store';
import { PlaceLink } from '@/features/navigation/places';
import { useSeenAfter, useSeenHere } from '@/features/navigation/visits';
import type { Org } from '@/generated/api-types';
import { reloadOrgTree } from '@/realtime/live';
import { useOrgTree } from '@/realtime/stores/tree';
import { boxClasses, Section } from '@/shell/Page';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { ThreadStateIcon } from '@/ui/ThreadState';
import { orgWork, workReason, type WorkThread } from './workModel';

/** Bound initial scanning to six rows per list; every remaining row opens in place without a read. */
const THREADS_SHOWN = 6;

/** Use the server's existing bounded recent-visits lookup; there are no per-thread detail requests. */
export const recentThreadsQuery = (orgId: string) => namesQuery({ q: '', kinds: ['thread'], orgId, limit: NAME_LIMIT_MAX });

export function WorkThreads({ rows, id }: { rows: readonly WorkThread[]; id: string }) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);
  const shown = expanded ? rows : rows.slice(0, THREADS_SHOWN);
  return <div className={boxClasses}>
    <ul id={id} className="flex flex-col divide-y divide-border">
      {shown.map(({ node, where }) => {
        const admin = adminAccessOf(node);
        return <li key={node.id} data-work-thread={node.id} className="flex items-start hover:bg-bg-subtle">
          <PlaceLink place={node} className="focus-inset flex min-w-0 flex-1 items-start gap-3 rounded-md px-4 py-3 text-text">
            <span className="flex h-5 shrink-0 items-center"><ThreadStateIcon display={node.thread?.display} /></span>
            <span className="flex min-w-0 flex-1 flex-col gap-1">
              <span data-thread-name className="font-medium leading-5 wrap-anywhere">{node.name}</span>
              <span data-thread-reason className="text-sm wrap-anywhere text-text-muted">{workReason(node, t)}</span>
              {where !== '' && <span className="text-xs wrap-anywhere text-text-subtle">{where}</span>}
            </span>
          </PlaceLink>
          {admin !== null && <span className="flex min-h-5 shrink-0 items-center pt-3 pr-4"><AsAdminMark of={admin.place} responsive /></span>}
        </li>;
      })}
    </ul>
    {rows.length > THREADS_SHOWN && <div className="border-t border-border px-4 py-2">
      <Button size="sm" variant="ghost" aria-expanded={expanded} aria-controls={id} onClick={() => setExpanded((value) => !value)}>
        {expanded ? t('places.org.work.showLess') : t('places.org.work.showMore', { count: rows.length - THREADS_SHOWN })}
      </Button>
    </div>}
  </div>;
}

/** Each tab mounts only its own content; this reader exists solely on the visible Overview. */
export function OrgWork({ org }: { org: Org }) {
  const { t } = useTranslation();
  const tree = useOrgTree(org.org_id);
  const visits = useQuery(recentThreadsQuery(org.org_id));
  const seen = useSeenHere();
  const openedSince = useSeenAfter(visits.dataUpdatedAt);
  const work = useMemo(() => orgWork(tree, org.id, visits.data ?? [], visits.isError ? seen : openedSince), [tree, org.id, visits.data, visits.isError, seen, openedSince]);
  const empty = work.waiting.length === 0 && work.recent.length === 0;
  return <div data-org-work className="flex flex-col gap-6">
    {work.waiting.length > 0 && <Section id={`waiting-work-${org.id}`} title={t('places.org.work.waiting')}>
      <WorkThreads key={org.id} id={`waiting-threads-${org.id}`} rows={work.waiting} />
    </Section>}
    <Section id={`recent-work-${org.id}`} title={t('places.org.work.recent')}>
      {work.recent.length > 0 && <WorkThreads key={org.id} id={`recent-threads-${org.id}`} rows={work.recent} />}
      {((!tree.loaded && tree.error === null) || visits.isPending) && <SkeletonRows rows={2} />}
      {tree.error !== null && <ErrorState compact error={tree.error} onRetry={() => reloadOrgTree(org.org_id)} />}
      {visits.isError && <ErrorState compact title={t('places.org.work.failedRead')} error={visits.error} retrying={visits.isRefetching} onRetry={() => void visits.refetch()} />}
      {tree.loaded && !visits.isPending && !visits.isError && tree.error === null && work.recent.length === 0 && <EmptyState compact
        title={t(empty ? 'places.org.work.empty' : 'places.org.work.noOtherRecents')}
        action={empty ? <Button size="sm" action="search" onClick={() => openFinder()}>{t('places.org.work.findThread')}</Button> : undefined}
      />}
    </Section>
  </div>;
}
