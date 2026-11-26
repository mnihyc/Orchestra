/** The full graph, in the main area where the document was: depth, kinds and likeness in meaning above it, and the nodes beside it. */
import { useQuery } from '@tanstack/react-query';
import { ChevronDown, Network } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { documentQuery } from '@/api/documents';
import { graphQuery, type GraphOptions } from '@/api/links';
import { GraphView } from '@/features/links/GraphView';
import { useViewChoice } from '@/lib/viewChoices';
import { useScreenHeading } from '@/shell/screen';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Menu } from '@/ui/Menu';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Skeleton } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { Switch } from '@/ui/Switch';
import { useDeviceChoiceGroups } from '@/ui/DeviceChoiceFacts';
import { FactDisclosureBody } from '@/ui/FactDisclosure';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';

/** What the address holds of the graph. */
export interface GraphSearch {
  depth?: number | undefined;
  kinds?: string | undefined;
  meaning?: boolean | undefined;
  center?: string | undefined;
}

const KINDS = ['document', 'chat', 'thread'] as const;
type Kind = (typeof KINDS)[number];

/** The room above and below the graph that the canvas leaves, so that the whole page fits one screen. */
const CHROME_HEIGHT = 260;
const MIN_HEIGHT = 320;
const MAX_HEIGHT = 760;

function useGraphHeight(): number {
  const [height, setHeight] = useState(() => typeof window === 'undefined' ? MIN_HEIGHT : window.innerHeight);
  useEffect(() => {
    const change = () => setHeight(window.innerHeight);
    window.addEventListener('resize', change);
    return () => window.removeEventListener('resize', change);
  }, []);
  return Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, height - CHROME_HEIGHT));
}

/** The kinds the address names, all of them when it names none. */
function kindsOf(text: string | undefined): Kind[] {
  const named = (text ?? '').split(',').filter((word): word is Kind => (KINDS as readonly string[]).includes(word));
  return named.length === 0 ? [...KINDS] : KINDS.filter((kind) => named.includes(kind));
}

export function GraphPage({ documentId, search, onSearch }: { documentId: string; search: GraphSearch; onSearch: (change: GraphSearch) => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const document = useQuery(documentQuery(documentId));
  const name = document.data?.title ?? '';
  // The last choices are where the next visit begins, as for any list; what the address names is what this visit shows.
  const [storedDepth, rememberDepth, depthReceipt] = useViewChoice<number>('links', 'graph-depth', 1);
  const [storedKinds, rememberKinds, kindsReceipt] = useViewChoice<string>('links', 'graph-kinds', '');
  const [storedMeaning, rememberMeaning, meaningReceipt] = useViewChoice<boolean>('links', 'graph-meaning', false);
  const asked = search.depth ?? storedDepth;
  const depth = asked === 2 || asked === 3 ? asked : 1;
  const kinds = kindsOf(search.kinds ?? storedKinds);
  const meaning = search.meaning ?? storedMeaning;
  const depthFacts = useDeviceChoiceGroups(depthReceipt === undefined ? undefined : { ...depthReceipt, effective: depth, origin: search.depth !== undefined ? 'address' : depth !== storedDepth ? 'default' : depthReceipt.origin,
    saved_choice_availability: typeof depthReceipt.saved === 'number' && ![1, 2, 3].includes(depthReceipt.saved) ? 'unavailable' : depthReceipt.saved_choice_availability }, formatNumber(depth, locale), typeof depthReceipt?.saved === 'number' && [1, 2, 3].includes(depthReceipt.saved) ? formatNumber(depthReceipt.saved, locale) : undefined);
  const kindWords = (value: string) => kindsOf(value).map((kind) => t(`links.kinds.${kind}`)).join(' · ');
  const kindsFacts = useDeviceChoiceGroups(kindsReceipt === undefined ? undefined : { ...kindsReceipt, effective: kinds, origin: search.kinds !== undefined ? 'address' : kindsReceipt.origin }, kindWords(search.kinds ?? storedKinds), typeof kindsReceipt?.saved === 'string' ? kindWords(kindsReceipt.saved) : undefined);
  const meaningFacts = useDeviceChoiceGroups(meaningReceipt === undefined ? undefined : { ...meaningReceipt, effective: meaning, origin: search.meaning !== undefined ? 'address' : meaningReceipt.origin }, t(meaning ? 'common.on' : 'common.off'), typeof meaningReceipt?.saved === 'boolean' ? t(meaningReceipt.saved ? 'common.on' : 'common.off') : undefined);
  const deviceGroups = [
    ...depthFacts.groups.map((group) => ({ ...group, key: 'graph-depth', title: t('links.graph.depth') })),
    ...kindsFacts.groups.map((group) => ({ ...group, key: 'graph-kinds', title: t('links.graph.kinds') })),
    ...meaningFacts.groups.map((group) => ({ ...group, key: 'graph-meaning', title: t('links.graph.meaning') })),
  ];
  const deviceFragment = depthFacts.fragment ?? kindsFacts.fragment ?? meaningFacts.fragment;
  const options: GraphOptions = { depth, kinds, meaning, center: search.center ?? null };
  const read = useQuery(graphQuery(documentId, options));
  const height = useGraphHeight();
  useScreenHeading({
    title: t('links.graph.title', { name }), icon: Network, leaf: true,
    ...(document.data === undefined ? {} : { nodeId: document.data.node.id, path: document.data.path }),
    trail: [{ label: name === '' ? t('links.title') : name, to: `/doc/${documentId}` }],
    views: { ready: document.data !== undefined },
  });
  const setKind = (kind: Kind, on: boolean) => {
    const next = KINDS.filter((each) => (each === kind ? on : kinds.includes(each)));
    const written = next.length === 0 || next.length === KINDS.length ? '' : next.join(',');
    rememberKinds(written);
    onSearch({ kinds: written === '' ? undefined : written });
  };
  return <div className="@container flex min-h-0 min-w-0 flex-1 flex-col gap-3 overflow-y-auto p-4" data-graph-page="">
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2" role="group" aria-label={t('links.graph.toolbar')}>
      <div className="flex items-center gap-2">
        <span className="text-sm text-text-muted" aria-hidden>{t('links.graph.depth')}</span>
        <SegmentedControl<'1' | '2' | '3'> size="sm" label={t('links.graph.depth')} value={String(depth) as '1' | '2' | '3'}
          onValueChange={(next) => { rememberDepth(Number(next)); onSearch({ depth: Number(next) }); }}
          segments={[{ value: '1', name: '1' }, { value: '2', name: '2' }, { value: '3', name: '3' }]} />
      </div>
      <Menu label={t('links.graph.kinds')} align="start" trigger={<Button size="sm">{t('links.graph.kinds')}<ChevronDown aria-hidden className="size-4" /></Button>}
        facts={<FactDisclosureBody groups={deviceGroups} />}
        entries={KINDS.map((kind) => ({ type: 'check' as const, id: kind, label: t(`links.kinds.${kind}`), checked: kinds.includes(kind), onCheckedChange: (on: boolean) => setKind(kind, on) }))} />
      <Switch label={t('links.graph.meaning')} checked={meaning} onCheckedChange={(on) => { rememberMeaning(on); onSearch({ meaning: on }); }} />
      {deviceFragment !== undefined && <span className="text-sm text-text-muted">{deviceFragment}</span>}
      {search.center !== undefined && <Button size="sm" variant="ghost" onClick={() => onSearch({ center: undefined })}>{t('links.graph.current')}</Button>}
    </div>
    {read.isPending ? <div aria-busy="true" style={{ height }}><Skeleton className="h-full w-full" /></div>
      : read.isError ? <ErrorAlert error={read.error} action={<Button size="sm" onClick={() => void read.refetch()}>{t('common.tryAgain')}</Button>} />
      : <>
        {meaning && read.data.meaning !== 'ready' && <p className="text-sm text-text-muted" role="status">{t(`related.meaning.${read.data.meaning}`)}</p>}
        {read.data.nodes.length <= 1 ? <StateLine title={t('links.graph.empty')} /> : <GraphView graph={read.data} height={height} listed="visible" onCenter={(center) => onSearch({ center })} />}
        {read.data.truncated && <p className="text-sm text-text-muted" role="status">{t('links.graph.limited', { count: read.data.nodes.length })}</p>}
      </>}
  </div>;
}
