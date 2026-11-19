/** The Links section of a document's right bar: what refers to it, what it refers to, what is related, and the graph around it.
 *
 * It replaces the separate Related section. Four tabs on one edge store; a tab the document switches off is not
 * drawn, and with all four off the section is not offered. The counts are the rows already read: a count that
 * reaches more pages says so with a plus, and nothing counts a row the reader may not see.
 */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useParams } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import { documentLinksQuery, referencesQuery } from '@/api/links';
import { listState } from '@/features/links/listState';
import { GraphTab } from '@/features/links/GraphTab';
import { ReferencedBy } from '@/features/links/ReferencedBy';
import { References } from '@/features/links/References';
import { RelatedTab } from '@/features/links/RelatedTab';
import { useViewInstance } from '@/features/workbench/instance';
import type { ViewProps } from '@/features/workbench/views';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { LinksTab } from '@/generated/api-types';
import { useViewChoice } from '@/lib/viewChoices';
import { useDocumentState } from '@/realtime/stores/document';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { Tabs, type TabDefinition } from '@/ui/Tabs';
import { Tooltip } from '@/ui/Tooltip';

const LOADING_ROWS = 3;

/** Two of the tabs have a short word for a bar too narrow for the long one; the long word stays in the tooltip. */
const SHORT: Partial<Record<LinksTab, 'links.tabs.cited_by' | 'links.tabs.cites'>> = { referenced_by: 'links.tabs.cited_by', references: 'links.tabs.cites' };

export function LinksView(_props: ViewProps) {
  const params = useParams({ strict: false });
  const instance = useViewInstance();
  const documentId = instance === null ? params.documentId : instance.target?.kind === 'document' ? instance.target.id : undefined;
  return documentId === undefined ? null : <LinksDocument key={documentId} documentId={documentId} />;
}

function LinksDocument({ documentId }: { documentId: string }) {
  const state = useDocumentState(documentId);
  const shown = useViewShown();
  if (state.ended !== null || state.error !== null || state.document?.deleted === true) return null;
  if (!state.loaded || state.document === null) return <SkeletonRows rows={LOADING_ROWS} />;
  return <LinksPanel documentId={documentId} seq={state.document.seq} enabled={shown} />;
}

/** The count a tab carries: the rows read, with a plus where more pages follow. */
function Count({ rows, more }: { rows: number | undefined; more: boolean }) {
  if (rows === undefined || rows === 0) return null;
  return more ? <Badge>{`${rows}+`}</Badge> : <Badge count={rows} />;
}

export function LinksPanel({ documentId, seq, enabled = true }: { documentId: string; seq: number; enabled?: boolean }) {
  const { t } = useTranslation();
  const tabs = useQuery({ ...documentLinksQuery(documentId), enabled });
  const shown: readonly LinksTab[] = tabs.data?.shown ?? [];
  const incoming = useInfiniteQuery({ ...referencesQuery(documentId, 'in'), enabled: enabled && shown.includes('referenced_by') });
  const outgoing = useInfiniteQuery({ ...referencesQuery(documentId, 'out'), enabled: enabled && shown.includes('references') });
  // The tab last chosen on this device is where a new visit begins; a choice made now holds even where none can be kept.
  const [stored, remember] = useViewChoice<string>('links', 'tab', 'referenced_by');
  const [picked, setPicked] = useState<string | null>(null);
  const chosen = picked ?? stored;
  const choose = (tab: string) => { setPicked(tab); remember(tab); };
  if (!enabled) return null;
  if (tabs.isPending) return <SkeletonRows rows={LOADING_ROWS} />;
  if (tabs.isError) return <ErrorAlert error={tabs.error} action={<Button size="sm" onClick={() => void tabs.refetch()}>{t('common.tryAgain')}</Button>} />;
  if (shown.length === 0) return null;
  const active: LinksTab = shown.find((tab) => tab === chosen) ?? shown[0] ?? 'referenced_by';
  const counted = (tab: LinksTab) => {
    const query = tab === 'referenced_by' ? incoming : tab === 'references' ? outgoing : null;
    return query === null ? null : <Count rows={query.data?.pages.reduce((sum, page) => sum + page.items.length, 0)} more={query.hasNextPage} />;
  };
  const label = (tab: LinksTab) => {
    const full = t(`links.tabs.${tab}`);
    const short = SHORT[tab];
    return <Tooltip content={full}><span className="inline-flex min-w-0 items-center gap-1.5">
      {short === undefined ? <span>{full}</span> : <><span className="hidden @[26rem]:inline">{full}</span><span className="@[26rem]:hidden">{t(short)}</span></>}
      {counted(tab)}
    </span></Tooltip>;
  };
  const panel = (tab: LinksTab) => {
    switch (tab) {
      case 'referenced_by': return <ReferencedBy documentId={documentId} list={listState(incoming)} />;
      case 'references': return <References list={listState(outgoing)} />;
      case 'related': return <RelatedTab documentId={documentId} seq={seq} enabled={enabled} />;
      case 'graph': return <GraphTab documentId={documentId} enabled={enabled} />;
    }
  };
  const definitions: TabDefinition<LinksTab>[] = shown.map((tab) => ({ value: tab, label: label(tab), menuLabel: t(`links.tabs.${tab}`), panel: <div className="min-w-0 p-2">{panel(tab)}</div> }));
  return <div className="@container h-full min-w-0 overflow-y-auto" data-links-section="">
    <Tabs<LinksTab> label={t('links.tabs.label')} value={active} onValueChange={choose} tabs={definitions}
      listClassName="[&>[role=tablist]]:flex-nowrap [&>[role=tablist]]:overflow-x-auto [&_button]:px-2" />
  </div>;
}
