/** Shared viewport geometry for popovers, inspections and the delegated full-value tooltip. */
import type { CSSProperties } from 'react';

/** Geometry shared by floating primitives; their inner list or card retains its native scrolling. */
export const floatingClasses = 'floating-layer';
/** A surface is hidden while its real trigger is outside its clipping boundary. */
export const floatingAttachment = { hideWhenDetached: true } as const;
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


/** Prefer above when the surface fits there; otherwise use the larger clear side of its trigger. */
export function floatingRoom(anchor: { top: number; bottom: number }, viewportHeight: number, surfaceHeight: number): { above: boolean; height: number } {
  const up = Math.max(0, anchor.top - FLOATING_GAP - FLOATING_EDGE);
  const down = Math.max(0, viewportHeight - anchor.bottom - FLOATING_GAP - FLOATING_EDGE);
  const above = surfaceHeight <= up || up > down;
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
