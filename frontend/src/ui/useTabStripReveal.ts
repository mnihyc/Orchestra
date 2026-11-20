/** Keep the selected complete tab inside its own scrolling strip, preserving manual browsing. */
import { useLayoutEffect, useRef, type RefObject } from 'react';

export function useTabStripReveal(activeId: string | null, tabIds: readonly string[], tabElementId: (id: string) => string, panelId: string): RefObject<HTMLDivElement | null> {
  const strip = useRef<HTMLDivElement>(null);
  const selection = useRef({ id: activeId, browsed: false });
  const tabOrder = JSON.stringify(tabIds);
  // Labels, icons and the available width can settle after selection.
  useLayoutEffect(() => {
    if (selection.current.id !== activeId) selection.current = { id: activeId, browsed: false };
    const intent = selection.current, element = strip.current;
    if (activeId === null || element === null || tabOrder === '[]') return;
    let live = true;
    const reveal = () => {
      if (!live || intent.browsed || element.clientWidth === 0) return;
      const tab = document.getElementById(tabElementId(activeId));
      if (tab === null || !element.contains(tab)) return;
      // The close/unsaved marks are siblings of the ARIA tab inside its strip root.
      let whole = tab;
      while (whole.parentElement !== null && whole.parentElement !== element) whole = whole.parentElement;
      const box = whole.getBoundingClientRect();
      const wrapped = element.clientHeight > 0 && element.scrollHeight > element.clientHeight;
      const viewport = element.getBoundingClientRect();
      const left = viewport.left + element.clientLeft, right = left + element.clientWidth;
      if (!(box.left < left && box.right > right)) {
        if (box.left < left) element.scrollLeft += box.left - left;
        else if (box.right > right) element.scrollLeft += box.right - right;
      }
      const top = viewport.top + element.clientTop;
      if (wrapped && (box.top < top - 0.5 || box.bottom > top + element.clientHeight + 0.5)) {
        const rowStart = element.scrollTop + box.top - top;
        // Interior rows align at the top; the final rows fill the viewport without synthetic space.
        element.scrollTop = Math.max(0, Math.min(rowStart, element.scrollHeight - element.clientHeight));
      }
    };
    // Let the person browse the strip until they choose a tab or return to its content.
    const browse = () => { intent.browsed = true; };
    const resume = () => {
      if (!intent.browsed) return;
      intent.browsed = false;
      reveal();
    };
    const panel = element.ownerDocument.getElementById(panelId);
    panel?.addEventListener('focusin', resume);
    panel?.addEventListener('pointerdown', resume);
    const select = (event: MouseEvent) => {
      if (event.target instanceof Element && event.target.closest('[role="tab"]')?.id === tabElementId(activeId)) {
        resume();
      }
    };
    // History restoration and browser scroll snapping can move a settled strip without resizing it.
    element.addEventListener('scroll', reveal, { passive: true });
    element.addEventListener('click', select);
    element.addEventListener('wheel', browse, { passive: true });
    element.addEventListener('pointerdown', browse, { passive: true });
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(reveal);
    observer?.observe(element);
    for (const child of Array.from(element.children)) observer?.observe(child);
    reveal();
    return () => {
      live = false;
      observer?.disconnect();
      panel?.removeEventListener('focusin', resume);
      panel?.removeEventListener('pointerdown', resume);
      element.removeEventListener('scroll', reveal);
      element.removeEventListener('click', select);
      element.removeEventListener('wheel', browse);
      element.removeEventListener('pointerdown', browse);
    };
  }, [activeId, panelId, tabElementId, tabOrder]);
  return strip;
}
