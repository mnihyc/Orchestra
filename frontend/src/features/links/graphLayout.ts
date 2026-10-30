/** Where the nodes of a graph stand: a force layout run to rest, so that a graph is drawn once and the same way each time.
 *
 * Nothing here draws or reads the page. The simulation starts from d3-force's own deterministic spiral and takes its
 * few random choices from a fixed generator, so a graph of the same nodes has the same shape on every device.
 */
import { forceCollide, forceLink, forceManyBody, forceSimulation, forceX, forceY, type SimulationLinkDatum, type SimulationNodeDatum } from 'd3-force';

export interface LayoutNode { id: string; distance: number; radius: number }
export interface LayoutEdge { from: string; to: string }
export interface Spot { x: number; y: number }

/** How far apart two nodes that refer to each other are drawn, before the whole is fitted to its room. */
const LINK_DISTANCE = 64;
/** How hard nodes push each other apart: enough that labels of a dozen neighbours do not overlap. */
const CHARGE = -220;
/** How firmly the graph is held round its centre, so that parts that refer to nothing do not drift away. */
const GRAVITY = 0.05;
/** The room kept between a node and its label, and between two nodes. */
const CLEARANCE = 4;
/** The space left free at the edges of the canvas for the nodes' labels. */
const MARGIN = 28;
/** A graph of two nodes is not drawn larger than this many times the distance it was laid out at. */
const MAX_SCALE = 1.6;
/** The most work, in nodes moved by one step, that one layout may take: a graph of the default size runs to rest well
 * within it, and a far larger one (the size is a setting) is laid out in fewer steps rather than holding the page. */
const WORK = 60_000;
/** Even the largest graph is moved this many steps, so that it is not left where it started. */
const LEAST_TICKS = 30;

interface Body extends SimulationNodeDatum { id: string; distance: number; radius: number }

/** A fixed generator: the layout never depends on `Math.random`. */
function generator(seed: number): () => number {
  let state = seed;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 4294967296;
  };
}

/** The place of each node, in pixels, inside a canvas of `width` by `height`; the centre node is held at its middle. */
export function layoutGraph(nodes: readonly LayoutNode[], edges: readonly LayoutEdge[], width: number, height: number): Map<string, Spot> {
  const bodies: Body[] = nodes.map((node) => ({ ...node, ...(node.distance === 0 ? { fx: 0, fy: 0 } : {}) }));
  const ids = new Set(bodies.map((body) => body.id));
  const links: SimulationLinkDatum<Body>[] = edges.filter((edge) => ids.has(edge.from) && ids.has(edge.to) && edge.from !== edge.to).map((edge) => ({ source: edge.from, target: edge.to }));
  const simulation = forceSimulation<Body>(bodies)
    .randomSource(generator(7))
    .force('link', forceLink<Body, SimulationLinkDatum<Body>>(links).id((body) => body.id).distance(LINK_DISTANCE).strength(0.7))
    .force('charge', forceManyBody<Body>().strength(CHARGE))
    .force('collide', forceCollide<Body>((body) => body.radius + CLEARANCE))
    .force('x', forceX<Body>(0).strength(GRAVITY))
    .force('y', forceY<Body>(0).strength(GRAVITY))
    .stop();
  // Run until the simulation would have stopped by itself, without waiting for any frame.
  const rest = Math.ceil(Math.log(simulation.alphaMin()) / Math.log(1 - simulation.alphaDecay()));
  const ticks = Math.min(rest, Math.max(LEAST_TICKS, Math.floor(WORK / Math.max(1, bodies.length))));
  for (let tick = 0; tick < ticks; tick += 1) simulation.tick();
  return fit(bodies, width, height);
}

/** Scales the layout to fill the canvas without going past its margins, with the centre node in the middle. */
function fit(bodies: readonly Body[], width: number, height: number): Map<string, Spot> {
  const reach = (axis: 'x' | 'y') => Math.max(1, ...bodies.map((body) => Math.abs(body[axis] ?? 0) + body.radius));
  const scale = Math.min(MAX_SCALE, Math.max(0.1, (width / 2 - MARGIN) / reach('x')), Math.max(0.1, (height / 2 - MARGIN) / reach('y')));
  return new Map(bodies.map((body): [string, Spot] => [body.id, { x: width / 2 + (body.x ?? 0) * scale, y: height / 2 + (body.y ?? 0) * scale }]));
}
