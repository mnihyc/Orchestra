/** The Graph tab of the Links section: a small graph around the document and the way to the full view. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { graphQuery } from '@/api/links';
import { GraphView } from '@/features/links/GraphView';
import { graphPreview } from '@/features/links/graphPreview';
import { useOpenLinked } from '@/features/links/open';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Skeleton } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';

/** The height of the small graph, in pixels. */
export const LOCAL_GRAPH_HEIGHT = 280;

export function GraphTab({ documentId, enabled }: { documentId: string; enabled: boolean }) {
  const { t, i18n } = useTranslation();
  const open = useOpenLinked();
  const [center, setCenter] = useState<string | null>(null);
  const read = useQuery({ ...graphQuery(documentId, { depth: 1, kinds: [], meaning: false, center }), enabled });
  if (read.isPending) return <div className="p-2" aria-busy="true"><div style={{ height: LOCAL_GRAPH_HEIGHT }}><Skeleton className="h-full w-full" /></div></div>;
  if (read.isError) return <div className="p-2"><ErrorAlert error={read.error} action={<Button size="sm" onClick={() => void read.refetch()}>{t('common.tryAgain')}</Button>} /></div>;
  const graph = graphPreview(read.data);
  const neighbours = Math.max(0, graph.nodes.length - 1);
  return <div className="flex min-w-0 flex-col gap-2 p-2" data-links-graph="">
    {graph.nodes.length <= 1 ? <StateLine title={t('links.graph.empty')} compact /> : <GraphView graph={graph} height={LOCAL_GRAPH_HEIGHT} listed="keyboard" onCenter={setCenter} />}
    {graph.truncated && <p className="text-sm text-text-muted" role="status">{t('links.graph.preview', { count: neighbours, number: formatNumber(neighbours, i18n.language) })}</p>}
    <div className="flex flex-wrap items-center gap-2">
      <Button size="sm" onClick={() => open(`/doc/${documentId}?graph=1`)}>{t('links.graph.open')}</Button>
      {center !== null && <Button size="sm" variant="ghost" onClick={() => setCenter(null)}>{t('links.graph.current')}</Button>}
    </div>
  </div>;
}
