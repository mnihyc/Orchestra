/** One graph with the list of its nodes: the picture for the pointer, the list for the keyboard and for screen readers.
 *
 * A click on a node of either re-centres the graph on it; Enter opens it. The list is the same nodes, nearest first,
 * with the place each lies in where two share a name.
 */
import { FileText, MessageSquareText, MessagesSquare } from 'lucide-react';
import { type KeyboardEvent, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { GraphCanvas, KIND_COLOUR } from '@/features/links/GraphCanvas';
import { useOpenLinked } from '@/features/links/open';
import { usePlaceLabels } from '@/features/navigation/placeLabels';
import type { GraphNode, LinkGraph } from '@/generated/api-types';
import { cx } from '@/ui/cx';

const ICONS = { document: FileText, chat: MessageSquareText, thread: MessagesSquare } as const;
const WORDS = { document: 'document', chat: 'chat', thread: 'thread' } as const;

/** Where a node opens. */
export function nodePath(node: Pick<GraphNode, 'kind' | 'id'>): string {
  return node.kind === 'document' ? `/doc/${node.id}` : node.kind === 'chat' ? `/chat/${node.id}` : `/thread/${node.id}`;
}

/** The centre a node would give the graph, in the words the request takes. */
export function centerOf(node: Pick<GraphNode, 'kind' | 'id'>): string {
  return `${WORDS[node.kind]}:${node.id}`;
}

export interface GraphViewProps {
  graph: LinkGraph;
  height: number;
  /** The list shows beside or under the picture; the section's small graph keeps it for the keyboard alone. */
  listed: 'visible' | 'keyboard';
  /** The reader chose a node as the new centre. */
  onCenter: (center: string) => void;
}

export function GraphView({ graph, height, listed, onCenter }: GraphViewProps) {
  const { t } = useTranslation();
  const open = useOpenLinked();
  const base = useId();
  const [marked, setMarked] = useState<string | null>(null);
  const current = graph.nodes.find((node) => node.current) ?? graph.nodes[0];
  const ordered = useMemo(() => [...graph.nodes].sort((one, other) => one.distance - other.distance || one.name.localeCompare(other.name)), [graph.nodes]);
  const labels = usePlaceLabels(ordered.map((node) => ({ id: node.id, name: node.name, above: node.context })));
  const byId = (id: string) => graph.nodes.find((node) => node.id === id);
  const optionId = (id: string) => `${base}-${id}`;
  const choose = (id: string) => { const node = byId(id); if (node !== undefined && node.distance !== 0) onCenter(centerOf(node)); };
  const openNode = (id: string) => { const node = byId(id); if (node !== undefined) open(nodePath(node)); };
  const step = (event: KeyboardEvent<HTMLUListElement>) => {
    const at = marked === null ? -1 : ordered.findIndex((node) => node.id === marked);
    const move = (to: number) => { event.preventDefault(); const node = ordered[Math.min(Math.max(to, 0), ordered.length - 1)]; if (node !== undefined) setMarked(node.id); };
    if (event.key === 'ArrowDown') move(at + 1);
    else if (event.key === 'ArrowUp') move(at - 1);
    else if (event.key === 'Home') move(0);
    else if (event.key === 'End') move(ordered.length - 1);
    else if (event.key === 'Enter' && marked !== null) { event.preventDefault(); openNode(marked); }
    else if (event.key === ' ' && marked !== null) { event.preventDefault(); choose(marked); }
  };
  return <div className={cx('flex min-w-0 gap-3', listed === 'visible' ? 'flex-col @3xl:flex-row' : 'flex-col')}>
    <div className="min-w-0 flex-1">
      <GraphCanvas graph={graph} height={height} marked={marked} label={t('links.graph.canvas', { name: current?.name ?? '' })} onChoose={choose} onOpen={openNode} />
    </div>
    <ul role="listbox" tabIndex={0} aria-label={t('links.graph.nodes')} aria-activedescendant={marked === null ? undefined : optionId(marked)} onKeyDown={step} data-graph-nodes=""
      className={cx('flex min-w-0 flex-col gap-0.5 overflow-y-auto', listed === 'visible' ? 'max-h-64 @3xl:max-h-none @3xl:w-72 @3xl:shrink-0' : 'sr-only focus:not-sr-only focus:max-h-40')}>
      {ordered.map((node) => {
        const Icon = ICONS[node.kind];
        return <li key={node.id} id={optionId(node.id)} role="option" aria-selected={marked === node.id}
          onClick={() => { setMarked(node.id); choose(node.id); }}
          className={cx('flex min-h-(--row-height) min-w-0 cursor-default items-center gap-2 rounded-md px-2 py-1 text-base', marked === node.id ? 'bg-accent-subtle' : 'hover:bg-bg-subtle')}>
          <Icon aria-hidden className="size-4 shrink-0" style={{ color: `var(${KIND_COLOUR[node.kind]})` }} />
          <span className="min-w-0 flex-1 truncate" title={labels.get(node.id) ?? node.name}>{labels.get(node.id) ?? node.name}</span>
          <span className="shrink-0 text-sm text-text-muted">{node.current ? t('links.graph.current') : t('links.graph.distance', { count: node.distance })}</span>
        </li>;
      })}
    </ul>
  </div>;
}
