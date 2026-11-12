/** Shared viewport geometry for popovers, inspections and the delegated full-value tooltip. */
import type { CSSProperties } from 'react';

/** Geometry shared by floating primitives; their inner list or card retains its native scrolling. */
export const floatingClasses = 'floating-layer';
/** Hide detached triggers and keep partially clipped anchors from pulling a surface outside the viewport. */
export const floatingAttachment = { hideWhenDetached: true, sticky: 'always' } as const;
/** The same small trigger gap used by all inspection cards. */
export const FLOATING_GAP = 6;
/** One design-system spacing step keeps a surface clear of each viewport edge. */
export const FLOATING_EDGE = 8;
/** Non-Radix editor bubbles use the same collision room as the shared popover. */
export const floatingPlacement = { flip: { padding: FLOATING_EDGE }, shift: { padding: FLOATING_EDGE, crossAxis: true } } as const;

/** Clamp a fixed surface using its own rendered size; floatingClasses makes both axes feasible. */
export function floatingPoint(left: number, top: number, above = false): CSSProperties {
  const edge = 'var(--spacing-2)';
  const x = `clamp(${edge},${left}px,calc(100dvw - ${edge}))`;
  const y = `clamp(${edge},${top}px,calc(100dvh - ${edge}))`;
  // Translation percentages use this surface's actual border box, so text, zoom and late content
  // need no guessed width/height or resize-time measurement.
  return { left: x, top: y, translate: `clamp(calc(${edge} - ${x}),0px,calc(100dvw - ${edge} - ${x} - 100%)) clamp(calc(${edge} - ${y}),${above ? '-100%' : '0px'},calc(100dvh - ${edge} - ${y} - 100%))` };
}


/** The room above and below an anchor inside a viewport, after the gap to the anchor and the edge of the screen. */
function roomAround(anchor: { top: number; bottom: number }, viewport: { top: number; bottom: number }, gap: number): { up: number; down: number } {
  return {
    up: Math.max(0, anchor.top - viewport.top - gap - FLOATING_EDGE),
    down: Math.max(0, viewport.bottom - anchor.bottom - gap - FLOATING_EDGE),
  };
}

/** Prefer the named side (above, for a hint) when the surface fits there; otherwise use the larger clear side of its trigger. */
export function floatingRoom(anchor: { top: number; bottom: number }, viewportHeight: number, surfaceHeight: number,
  prefer: 'above' | 'below' = 'above', gap = FLOATING_GAP): { above: boolean; height: number } {
  const { up, down } = roomAround(anchor, { top: 0, bottom: viewportHeight }, gap);
  const above = prefer === 'above' ? surfaceHeight <= up || up > down : surfaceHeight > down && (surfaceHeight <= up || up > down);
  return { above, height: above ? up : down };
}

/** How many rows a list in a floating layer promises to show before the layer looks for more room on the other side of what opened it. */
export const FLOATING_MIN_ROWS = 4;
/** The least a floating layer of no rows (a card) promises to show of itself, in CSS pixels: four rows' height. */
export const FLOATING_MIN_HEIGHT = 128;

/**
 * The side and the height of a floating layer above or below what opened it, for the content it has now. One rule for
 * every layer that opens that way (Combobox, Menu, Select, Popover and the cards).
 *
 * It opens on the side that holds all of it, the side it prefers first (below, for a list; above where its caller says
 * so); when neither side does, on the larger one. Once it stands somewhere it stays there, whatever its content does,
 * while that side still shows `least` of it (the rows it promises and the controls around them); it changes sides only
 * when that side no longer does and the other has more room, so a list that loads, narrows or widens does not move
 * under a person who is choosing, and does not stay a sliver with a whole screen's room on the other side.
 */
export function layerRoom(input: {
  anchor: { top: number; bottom: number };
  viewport: { top: number; bottom: number };
  /** The layer's whole height, if nothing held it. */
  natural: number;
  /** The least worth showing of it: never more than `natural`. */
  least: number;
  /** The side it stands on now, once it has stood anywhere. */
  now?: 'above' | 'below';
  /** The side it opens on when all of it fits on both. */
  prefer?: 'above' | 'below';
  gap: number;
}): { above: boolean; height: number } {
  const { up, down } = roomAround(input.anchor, input.viewport, input.gap);
  if (input.now !== undefined) {
    const have = input.now === 'above' ? up : down, other = input.now === 'above' ? down : up;
    if (have >= Math.min(input.least, input.natural) || have >= other) return { above: input.now === 'above', height: have };
  }
  const above = (input.prefer ?? 'below') === 'above'
    ? input.natural <= up || up > down
    : input.natural > down && (input.natural <= up || up > down);
  return { above, height: above ? up : down };
}


/** Reveal keyboard focus only within its nearest floating surface and its inner list. */
export function revealFloatingFocus(element: HTMLElement): void {
  const layer = element.closest('.floating-layer,[data-radix-popper-content-wrapper] > *');
  if (layer === null) return;
  for (let parent = element.parentElement; parent !== null; parent = parent.parentElement) {
    const style = getComputedStyle(parent), frame = parent.getBoundingClientRect(), box = element.getBoundingClientRect();
    if (['auto', 'scroll'].includes(style.overflowY) && parent.clientHeight > 0) {
      const top = frame.top + parent.clientTop, bottom = top + parent.clientHeight;
      if (box.top < top) parent.scrollTop += box.top - top;
      else if (box.bottom > bottom) parent.scrollTop += box.bottom - bottom;
    }
    if (['auto', 'scroll'].includes(style.overflowX) && parent.clientWidth > 0) {
      const left = frame.left + parent.clientLeft, right = left + parent.clientWidth;
      if (box.left < left) parent.scrollLeft += box.left - left;
      else if (box.right > right) parent.scrollLeft += box.right - right;
    }
    if (parent === layer) break;
  }
}
