/** The graph on a canvas: nodes coloured by kind with the chart colours, the current document ringed, references as arrows and likeness in meaning as dashes.
 *
 * It only draws what it is given and says which node a pointer chose; the list beside it is the keyboard's way
 * round the same nodes. Colours are read from the page's tokens each time it is drawn, and again when the theme
 * changes, so no colour is written here.
 */
import { type RefObject, useEffect, useMemo, useRef, useState } from 'react';

import type { LinkGraph } from '@/generated/api-types';
import { layoutGraph, type Spot } from '@/features/links/graphLayout';
import { placeGraphLabels } from '@/features/links/graphLabels';

/** The kinds a graph draws, each with the chart colour it keeps. */
export const KIND_COLOUR: Readonly<Record<LinkGraph['nodes'][number]['kind'], string>> = {
  document: '--chart-1',
  chat: '--chart-3',
  thread: '--chart-2',
};

/** Radius of an ordinary node, and of the centre and the current document. */
const NODE_RADIUS = 6;
const CENTER_RADIUS = 9;
/** How close, beyond a node's edge, a pointer still means that node. */
const HIT_SLOP = 6;
/** Longest label, in characters, before it is cut. */
const LABEL_CHARS = 22;
/** Past this many nodes only those next to the centre, and the chosen one, carry their labels. */
const LABELS_ALL_UNTIL = 40;
/** The measured 12px text gets a full 16px line for collision placement. */
const LABEL_HEIGHT = 16;
const ARROW = 6;

/** Rebuilds a drawing when the theme changes: by the page's own switch or by the system's. */
function useThemeVersion(): number {
  const [version, setVersion] = useState(0);
  useEffect(() => {
    const bump = () => setVersion((before) => before + 1);
    const observer = new MutationObserver(bump);
    observer.observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'class'] });
    const media = typeof matchMedia === 'function' ? matchMedia('(prefers-color-scheme: dark)') : null;
    media?.addEventListener('change', bump);
    return () => { observer.disconnect(); media?.removeEventListener('change', bump); };
  }, []);
  return version;
}

function useWidth(target: RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const element = target.current;
    if (element === null) return;
    // The first measure comes with the next frame, not inside this effect: it is a reading of the page, not a derived value.
    const first = requestAnimationFrame(() => setWidth(element.clientWidth));
    if (typeof ResizeObserver === 'undefined') return () => cancelAnimationFrame(first);
    const observer = new ResizeObserver(() => setWidth(element.clientWidth));
    observer.observe(element);
    return () => { cancelAnimationFrame(first); observer.disconnect(); };
  }, [target]);
  return width;
}

const clip = (text: string) => (text.length > LABEL_CHARS ? `${text.slice(0, LABEL_CHARS - 1)}…` : text);

export interface GraphCanvasProps {
  graph: LinkGraph;
  /** What the picture is of, for a screen reader. */
  label: string;
  height: number;
  /** The node the list has marked; ringed in the accent colour. */
  marked: string | null;
  /** A node was chosen with the pointer. */
  onChoose: (id: string) => void;
  /** Enter was pressed on the picture: open the marked node, or the centre. */
  onOpen: (id: string) => void;
}

export function GraphCanvas({ graph, label, height, marked, onChoose, onOpen }: GraphCanvasProps) {
  const frame = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const width = useWidth(frame);
  const theme = useThemeVersion();
  const spots = useMemo((): Map<string, Spot> => width === 0 ? new Map<string, Spot>() : layoutGraph(
    graph.nodes.map((node) => ({ id: node.id, distance: node.distance, radius: node.current || node.distance === 0 ? CENTER_RADIUS : NODE_RADIUS })),
    graph.edges.map((edge) => ({ from: edge.from, to: edge.to })),
    width,
    height,
  ), [graph, width, height]);

  useEffect(() => {
    const element = canvas.current;
    const context = element?.getContext('2d') ?? null;
    if (element === null || context === null || width === 0) return;
    const ratio = window.devicePixelRatio || 1;
    element.width = Math.round(width * ratio);
    element.height = Math.round(height * ratio);
    context.setTransform(ratio, 0, 0, ratio, 0, 0);
    context.clearRect(0, 0, width, height);
    const style = getComputedStyle(element);
    const token = (name: string, fallback: string) => style.getPropertyValue(name).trim() || fallback;
    const line = token('--border-strong', '#8884');
    const text = token('--text', '#222');
    const ground = token('--bg', '#fff');
    const accent = token('--accent', '#46f');
    const byId = new Map(graph.nodes.map((node) => [node.id, node]));
    const radius = (id: string) => { const node = byId.get(id); return node?.current === true || node?.distance === 0 ? CENTER_RADIUS : NODE_RADIUS; };
    for (const edge of graph.edges) {
      const from = spots.get(edge.from), to = spots.get(edge.to);
      if (from === undefined || to === undefined) continue;
      context.beginPath();
      context.strokeStyle = line;
      context.lineWidth = 1;
      context.setLineDash(edge.kind === 'meaning' ? [4, 4] : []);
      context.moveTo(from.x, from.y);
      context.lineTo(to.x, to.y);
      context.stroke();
      if (edge.kind === 'reference') {
        const angle = Math.atan2(to.y - from.y, to.x - from.x);
        const tipX = to.x - Math.cos(angle) * (radius(edge.to) + 1), tipY = to.y - Math.sin(angle) * (radius(edge.to) + 1);
        context.setLineDash([]);
        context.beginPath();
        context.fillStyle = line;
        context.moveTo(tipX, tipY);
        context.lineTo(tipX - Math.cos(angle - 0.45) * ARROW, tipY - Math.sin(angle - 0.45) * ARROW);
        context.lineTo(tipX - Math.cos(angle + 0.45) * ARROW, tipY - Math.sin(angle + 0.45) * ARROW);
        context.closePath();
        context.fill();
      }
    }
    context.setLineDash([]);
    context.font = '12px system-ui, sans-serif';
    context.textAlign = 'left';
    context.textBaseline = 'top';
    const labelled = graph.nodes.length <= LABELS_ALL_UNTIL;
    const labels = placeGraphLabels(graph.nodes.flatMap((node) => {
      const spot = spots.get(node.id);
      if (spot === undefined) return [];
      return [{ ...spot, id: node.id, radius: radius(node.id), width: context.measureText(clip(node.name)).width, height: LABEL_HEIGHT,
        priority: node.id === marked ? -2 : node.distance === 0 ? -1 : node.distance, labelled: labelled || node.distance <= 1 || marked === node.id }];
    }), width, height);
    for (const node of graph.nodes) {
      const spot = spots.get(node.id);
      if (spot === undefined) continue;
      const size = radius(node.id);
      context.beginPath();
      context.arc(spot.x, spot.y, size, 0, Math.PI * 2);
      context.fillStyle = token(KIND_COLOUR[node.kind], accent);
      context.fill();
      if (node.current) {
        context.lineWidth = 2;
        context.strokeStyle = text;
        context.stroke();
      }
      if (marked === node.id) {
        context.beginPath();
        context.arc(spot.x, spot.y, size + 4, 0, Math.PI * 2);
        context.lineWidth = 2;
        context.strokeStyle = accent;
        context.stroke();
      }
      const labelAt = labels.get(node.id);
      if (labelAt !== undefined) {
        const words = clip(node.name);
        context.lineWidth = 3;
        context.strokeStyle = ground;
        context.strokeText(words, labelAt.x, labelAt.y);
        context.fillStyle = text;
        context.fillText(words, labelAt.x, labelAt.y);
      }
    }
  }, [graph, spots, marked, width, height, theme]);

  const nodeAt = (x: number, y: number): string | null => {
    let best: string | null = null;
    let nearest = Infinity;
    for (const node of graph.nodes) {
      const spot = spots.get(node.id);
      if (spot === undefined) continue;
      const reach = (node.current || node.distance === 0 ? CENTER_RADIUS : NODE_RADIUS) + HIT_SLOP;
      const gap = Math.hypot(spot.x - x, spot.y - y);
      if (gap <= reach && gap < nearest) { best = node.id; nearest = gap; }
    }
    return best;
  };
  const center = graph.nodes.find((node) => node.distance === 0)?.id ?? graph.center;

  return <div ref={frame} className="relative min-w-0 overflow-hidden rounded-md border border-border bg-surface" style={{ height }}>
    <canvas ref={canvas} role="img" aria-label={label} tabIndex={0} data-graph-canvas="" style={{ width: '100%', height }} className="block cursor-default focus-visible:outline-2 focus-visible:outline-accent"
      onPointerDown={(event) => {
        const box = event.currentTarget.getBoundingClientRect();
        const hit = nodeAt(event.clientX - box.left, event.clientY - box.top);
        if (hit !== null && hit !== center) onChoose(hit);
      }}
      onPointerMove={(event) => {
        const box = event.currentTarget.getBoundingClientRect();
        const hit = nodeAt(event.clientX - box.left, event.clientY - box.top);
        event.currentTarget.style.cursor = hit !== null && hit !== center ? 'pointer' : 'default';
        event.currentTarget.title = graph.nodes.find((node) => node.id === hit)?.name ?? '';
      }}
      onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); onOpen(marked ?? center); } }} />
  </div>;
}
