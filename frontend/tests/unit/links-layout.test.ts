/** A graph is laid out once, the same way each time, inside its canvas, with its centre in the middle. */
import { expect, it } from 'vitest';

import { layoutGraph } from '@/features/links/graphLayout';
import { placeGraphLabels, type LabelNode } from '@/features/links/graphLabels';
import { graphPreview, PREVIEW_NEIGHBOURS } from '@/features/links/graphPreview';
import type { GraphNode, LinkGraph } from '@/generated/api-types';

const node = (id: string, distance: number) => ({ id, distance, radius: distance === 0 ? 9 : 6 });

it('holds the centre in the middle and every node inside the canvas', () => {
  const nodes = [node('c', 0), ...Array.from({ length: 24 }, (_, at) => node(`n${at}`, at < 8 ? 1 : 2))];
  const edges = nodes.slice(1).map((each) => ({ from: each.id, to: each.distance === 1 ? 'c' : 'n0' }));
  const spots = layoutGraph(nodes, edges, 400, 280);
  expect(spots.get('c')).toEqual({ x: 200, y: 140 });
  for (const spot of spots.values()) {
    expect(Number.isFinite(spot.x) && Number.isFinite(spot.y)).toBe(true);
    expect(spot.x).toBeGreaterThanOrEqual(0);
    expect(spot.x).toBeLessThanOrEqual(400);
    expect(spot.y).toBeGreaterThanOrEqual(0);
    expect(spot.y).toBeLessThanOrEqual(280);
  }
});

it('gives the same places to the same graph', () => {
  const nodes = [node('c', 0), node('a', 1), node('b', 1), node('d', 2)];
  const edges = [{ from: 'a', to: 'c' }, { from: 'c', to: 'b' }, { from: 'b', to: 'd' }];
  expect([...layoutGraph(nodes, edges, 320, 280)]).toEqual([...layoutGraph(nodes, edges, 320, 280)]);
});

it('keeps nodes apart and ignores edges to nodes that are not there', () => {
  const nodes = [node('c', 0), node('a', 1), node('b', 1)];
  const spots = layoutGraph(nodes, [{ from: 'a', to: 'ghost' }, { from: 'a', to: 'a' }], 320, 280);
  const [a, b] = [spots.get('a'), spots.get('b')];
  expect(Math.hypot((a?.x ?? 0) - (b?.x ?? 0), (a?.y ?? 0) - (b?.y ?? 0))).toBeGreaterThan(12);
});

it('lays out a graph of one node and an empty graph', () => {
  expect(layoutGraph([node('c', 0)], [], 300, 200).get('c')).toEqual({ x: 150, y: 100 });
  expect(layoutGraph([], [], 300, 200).size).toBe(0);
});

it('lays out a graph far larger than the default in a bounded number of steps and still places every node', () => {
  const nodes = [node('c', 0), ...Array.from({ length: 2999 }, (_, at) => node(`n${at}`, 1))];
  const edges = nodes.slice(1).map((each) => ({ from: each.id, to: 'c' }));
  const spots = layoutGraph(nodes, edges, 800, 600);
  expect(spots.size).toBe(3000);
  for (const spot of spots.values()) expect(Number.isFinite(spot.x) && Number.isFinite(spot.y)).toBe(true);
});

it('gives a crowded inspector only its centre and the nearest four neighbours without changing the full graph', () => {
  const center: GraphNode = { id: 'c', name: 'This document', kind: 'document', context: [], current: true, distance: 0 };
  const neighbours = Array.from({ length: 30 }, (_, at) => ({ ...center, id: `n${at}`, name: `Note ${at.toString().padStart(2, '0')}`, current: false, distance: at < 10 ? 2 : 1 }));
  const full: LinkGraph = { center: 'c', nodes: [...neighbours, center], edges: neighbours.map((each) => ({ from: each.id, to: 'c', kind: 'reference' })), truncated: false, meaning: 'off' };
  const preview = graphPreview(full);
  expect(preview.nodes.map((each) => each.id)).toEqual(['c', 'n10', 'n11', 'n12', 'n13']);
  expect(preview.nodes).toHaveLength(PREVIEW_NEIGHBOURS + 1);
  expect(preview.edges).toHaveLength(PREVIEW_NEIGHBOURS);
  expect(preview.edges.every((edge) => preview.nodes.some((each) => each.id === edge.from) && preview.nodes.some((each) => each.id === edge.to))).toBe(true);
  expect(preview.truncated).toBe(true);
  expect(full.nodes).toHaveLength(31);
  expect(full.edges).toHaveLength(30);
  expect(full.truncated).toBe(false);
});

it('keeps an already small graph intact, including the server truncation signal', () => {
  const graph: LinkGraph = { center: 'c', nodes: [], edges: [], truncated: true, meaning: 'off' };
  expect(graphPreview(graph)).toBe(graph);
});


it('keeps crowded graph labels inside the picture and clear of other names and every node', () => {
  for (const width of [366, 668]) {
    const nodes = [node('c', 0), ...Array.from({ length: 30 }, (_, at) => node(`n${at}`, 1))];
    const spots = layoutGraph(nodes, nodes.slice(1).map((each) => ({ from: each.id, to: 'c' })), width, 640);
    const labels: LabelNode[] = nodes.map((each) => {
      const spot = spots.get(each.id);
      if (spot === undefined) throw new Error(`Missing node ${each.id}`);
      return { ...spot, ...each, width: 108, height: 16, priority: each.distance, labelled: true };
    });
    const placed = placeGraphLabels(labels, width, 640);
    expect(placed.has('c')).toBe(true);
    expect(placed.size).toBeGreaterThan(5);
    const boxes = [...placed].map(([id, spot]) => ({ id, ...spot, right: spot.x + 108, bottom: spot.y + 16 }));
    for (const box of boxes) {
      expect(box.x).toBeGreaterThanOrEqual(0);
      expect(box.y).toBeGreaterThanOrEqual(0);
      expect(box.right).toBeLessThanOrEqual(width);
      expect(box.bottom).toBeLessThanOrEqual(640);
      for (const other of boxes) if (other.id !== box.id) {
        expect(box.x < other.right && box.right > other.x && box.y < other.bottom && box.bottom > other.y).toBe(false);
      }
      for (const dot of labels) expect(box.x < dot.x + dot.radius && box.right > dot.x - dot.radius && box.y < dot.y + dot.radius && box.bottom > dot.y - dot.radius).toBe(false);
    }
    expect(nodes).toHaveLength(31);
  }
});

it('keeps names available at the edge and gives a marked node the first free label position', () => {
  const edge: LabelNode = { id: 'edge', x: 20, y: 20, radius: 6, width: 120, height: 16, priority: 1, labelled: true };
  const chosen: LabelNode = { ...edge, id: 'chosen', priority: -1 };
  const placed = placeGraphLabels([edge, chosen], 180, 100);
  expect(placed.get('chosen')).toBeDefined();
  for (const spot of placed.values()) {
    expect(spot.x).toBeGreaterThanOrEqual(0);
    expect(spot.x + 120).toBeLessThanOrEqual(180);
    expect(spot.y).toBeGreaterThanOrEqual(0);
    expect(spot.y + 16).toBeLessThanOrEqual(100);
  }
});

it('keeps a far larger crowded graph bounded by available label space without losing its input nodes', () => {
  const nodes: LabelNode[] = Array.from({ length: 3000 }, (_, at) => ({ id: `n${at}`, x: 200, y: 150, radius: 6, width: 120, height: 16, priority: at, labelled: true }));
  const placed = placeGraphLabels(nodes, 400, 300);
  expect(placed.has('n0')).toBe(true);
  expect(placed.size).toBeGreaterThan(0);
  expect(placed.size).toBeLessThanOrEqual(Math.floor(400 * 300 / (120 * 16)));
  expect(nodes).toHaveLength(3000);
});
