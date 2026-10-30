/** Graph labels use free space beside their nodes; the node list keeps every name when the picture is crowded. */
import type { Spot } from '@/features/links/graphLayout';

export interface LabelNode extends Spot {
  id: string; radius: number; width: number; height: number; priority: number; labelled: boolean;
}
interface Rect { left: number; top: number; right: number; bottom: number }

/** Two text lines per cell keep each short label's collision lookup local. */
const CELL = 32;
/** Leave the canvas border and node outline clear of text. */
const PADDING = 4;
const GAP = 4;
const OUTLINE = 2;

const overlaps = (a: Rect, b: Rect) => a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;

/** Places useful labels first, trying a fixed number of local positions beside each node. */
export function placeGraphLabels(nodes: readonly LabelNode[], width: number, height: number): Map<string, Spot> {
  const occupied = new Map<string, Rect[]>();
  const cells = (box: Rect, visit: (key: string) => boolean): boolean => {
    for (let x = Math.floor(box.left / CELL); x <= Math.floor(box.right / CELL); x += 1) {
      for (let y = Math.floor(box.top / CELL); y <= Math.floor(box.bottom / CELL); y += 1) {
        if (!visit(`${x}:${y}`)) return false;
      }
    }
    return true;
  };
  const reserve = (box: Rect) => cells(box, (key) => {
    const inCell = occupied.get(key);
    if (inCell === undefined) occupied.set(key, [box]);
    else inCell.push(box);
    return true;
  });
  for (const node of nodes) reserve({ left: node.x - node.radius - OUTLINE, top: node.y - node.radius - OUTLINE, right: node.x + node.radius + OUTLINE, bottom: node.y + node.radius + OUTLINE });
  const result = new Map<string, Spot>();
  for (const node of [...nodes].sort((a, b) => a.priority - b.priority)) {
    if (!node.labelled) continue;
    const below = node.y + node.radius + GAP;
    const above = node.y - node.radius - GAP - node.height;
    const right = node.x + node.radius + GAP;
    const left = node.x - node.radius - GAP - node.width;
    const centered = node.x - node.width / 2;
    const middle = node.y - node.height / 2;
    const candidates: readonly (readonly [number, number])[] = [[centered, below], [centered, above], [right, middle], [left, middle], [right, below], [left, below], [right, above], [left, above]];
    for (const [x, y] of candidates) {
      const box = { left: Math.max(PADDING, Math.min(x, width - PADDING - node.width)), top: Math.max(PADDING, Math.min(y, height - PADDING - node.height)), right: 0, bottom: 0 };
      box.right = box.left + node.width;
      box.bottom = box.top + node.height;
      if (box.right > width - PADDING || box.bottom > height - PADDING) continue;
      if (!cells(box, (key) => !(occupied.get(key)?.some((other) => overlaps(box, other)) ?? false))) continue;
      result.set(node.id, { x: box.left, y: box.top });
      reserve({ left: box.left - OUTLINE, top: box.top - OUTLINE, right: box.right + OUTLINE, bottom: box.bottom + OUTLINE });
      break;
    }
  }
  return result;
}
