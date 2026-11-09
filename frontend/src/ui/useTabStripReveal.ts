/** Keep the selected complete tab inside its own scrolling strip, preserving manual browsing. */
import { useLayoutEffect, useRef, type RefObject } from 'react';

export function useTabStripReveal(activeId: string | null, tabIds: readonly string[], tabElementId: (id: string) => string): RefObject<HTMLDivElement | null> {
  const strip = useRef<HTMLDivElement>(null);
  const selection = useRef({ id: activeId, browsed: false });
  const tabOrder = JSON.stringify(tabIds);
  // Labels/icons and the available width can settle after selection. Reveal within this strip only;
  // a person browsing it manually keeps that position until a different tab is selected.
  useLayoutEffect(() => {
    if (selection.current.id !== activeId) selection.current = { id: activeId, browsed: false };
    const intent = selection.current, element = strip.current;
    if (activeId === null || element === null || tabOrder === '[]') return;
    let live = true;
    const reveal = () => {
      if (!live || element.clientWidth === 0) return;
      const tab = document.getElementById(tabElementId(activeId));
      if (tab === null || !element.contains(tab)) return;
      // The close/unsaved marks are siblings of the ARIA tab inside its strip root.
      let whole = tab;
      while (whole.parentElement !== null && whole.parentElement !== element) whole = whole.parentElement;
      const box = whole.getBoundingClientRect();
      const previousPadding = Number.parseFloat(element.style.paddingBottom) || 0;
      const wrapped = element.clientHeight > 0 && element.scrollHeight - previousPadding > element.clientHeight;
      // One real row of trailing room lets the final row reach the top too. Remove it when rows fit.
      const padding = wrapped ? box.height : 0;
      if (padding !== previousPadding) element.style.paddingBottom = `${padding}px`;
      if (intent.browsed) return;
      const viewport = element.getBoundingClientRect();
      const left = viewport.left + element.clientLeft, right = left + element.clientWidth;
      if (!(box.left < left && box.right > right)) {
        if (box.left < left) element.scrollLeft += box.left - left;
        else if (box.right > right) element.scrollLeft += box.right - right;
      }
      const top = viewport.top + element.clientTop;
      if (wrapped) element.scrollTop += box.top - top;
    };
    // A strip gesture may browse, drag a tab, or grab its scrollbar; only a new selection resumes reveal.
    const browse = () => { intent.browsed = true; };
    element.addEventListener('wheel', browse, { passive: true });
    element.addEventListener('pointerdown', browse, { passive: true });
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(reveal);
    observer?.observe(element);
    for (const child of Array.from(element.children)) observer?.observe(child);
    reveal();
    return () => {
      live = false;
      observer?.disconnect();
      element.removeEventListener('wheel', browse);
      element.removeEventListener('pointerdown', browse);
    };
  }, [activeId, tabElementId, tabOrder]);
  return strip;
}
