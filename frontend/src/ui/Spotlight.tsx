/** A frame around a real control. It neither intercepts the control nor reads/copies its content. */
import { useMemo, useSyncExternalStore } from 'react';
import { createPortal } from 'react-dom';

export interface SpotlightRect { left: number; top: number; width: number; height: number }
export interface SpotlightTarget { element: HTMLElement; rect: SpotlightRect }

/** Hidden, inert and closed-panel copies are never a tutorial target. */
export function findSpotlightTarget(selector: string, root: ParentNode = document): HTMLElement | null {
  return [...root.querySelectorAll<HTMLElement>(selector)].find((element) =>
    element.isConnected && element.closest('[hidden], [inert], [aria-hidden="true"]') === null &&
    getComputedStyle(element).visibility !== 'hidden' && element.getClientRects().length > 0,
  ) ?? null;
}

/** Measurements are an external subscription, so React reads one stable snapshot per actual change. */
function targetStore(selector: string | null) {
  let target: SpotlightTarget | null = null;
  return {
    snapshot: () => target,
    subscribe: (notify: () => void) => {
      if (selector === null) return () => undefined;
      const selected = selector;
      let observed: HTMLElement | null = null;
      const resized = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(measure);
      function measure() {
        const element = findSpotlightTarget(selected);
        if (element !== observed) {
          if (observed !== null) resized?.unobserve(observed);
          observed = element;
          if (observed !== null) resized?.observe(observed);
        }
        if (element === null) { if (target !== null) { target = null; notify(); } return; }
        const box = element.getBoundingClientRect();
        const rect = { left: box.left, top: box.top, width: box.width, height: box.height };
        if (target?.element === element && target.rect.left === rect.left && target.rect.top === rect.top && target.rect.width === rect.width && target.rect.height === rect.height) return;
        target = { element, rect };
        notify();
      }
      const changed = new MutationObserver(measure);
      changed.observe(document.body, { childList: true, subtree: true, attributes: true });
      window.addEventListener('resize', measure);
      document.addEventListener('scroll', measure, true);
      measure();
      return () => { changed.disconnect(); resized?.disconnect(); window.removeEventListener('resize', measure); document.removeEventListener('scroll', measure, true); };
    },
  };
}

export function useSpotlightTarget(selector: string | null): SpotlightTarget | null {
  const store = useMemo(() => targetStore(selector), [selector]);
  return useSyncExternalStore(store.subscribe, store.snapshot, () => null);
}

export function Spotlight({ target }: { target: SpotlightTarget | null }) {
  if (target === null) return null;
  // Four pixels of clearance follows the interface's small spacing. Clip to the viewport to avoid page overflow.
  const left = Math.max(0, target.rect.left - 4);
  const top = Math.max(0, target.rect.top - 4);
  const right = Math.min(window.innerWidth, target.rect.left + target.rect.width + 4);
  const bottom = Math.min(window.innerHeight, target.rect.top + target.rect.height + 4);
  if (right <= left || bottom <= top) return null;
  return createPortal(<div aria-hidden="true" data-testid="tutorial-frame" className="pointer-events-none fixed z-50 rounded-md border-2 border-accent shadow-md" style={{ left, top, width: right - left, height: bottom - top }} />, document.body);
}
