/** Sheets start below the current wrapped chrome, including banners, rather than covering its controls. */
import { useLayoutEffect, useState } from 'react';

const CHROME = '[data-workbench-chrome], [data-screen-titlebar]';

export function useChromeBottom(): number | undefined {
  const [bottom, setBottom] = useState<number>();
  useLayoutEffect(() => {
    let observed: HTMLElement | null = null;
    let frame: number | null = null;
    const observer = typeof ResizeObserver === 'function' ? new ResizeObserver(() => measure()) : null;
    const measure = () => {
      const candidates = Array.from(document.querySelectorAll<HTMLElement>(CHROME));
      const visible = candidates.filter((element) => element.getBoundingClientRect().height > 0);
      const chrome = visible.find((element) => element.hasAttribute('data-workbench-chrome')) ?? visible[0] ?? candidates[0] ?? null;
      if (chrome !== observed) {
        if (observed !== null) observer?.unobserve(observed);
        observed = chrome;
        if (chrome !== null) observer?.observe(chrome);
      }
      const box = chrome?.getBoundingClientRect();
      // Round outward by a CSS pixel so fractional text painting stays above the sheet.
      const next = box === undefined || box.height <= 0 ? undefined : Math.max(0, Math.ceil(box.bottom));
      setBottom((previous) => previous === next ? previous : next);
    };
    const schedule = () => {
      if (frame !== null) return;
      frame = requestAnimationFrame(() => { frame = null; measure(); });
    };
    const containsChrome = (node: Node) => node instanceof Element && (node.matches(CHROME) || node.querySelector(CHROME) !== null);
    // Desktop and phone navigation replace each other. Conversation mutations do not remeasure sheets.
    const mutations = new MutationObserver((records) => {
      if (records.some((record) => record.type === 'attributes' || [...record.addedNodes, ...record.removedNodes].some(containsChrome))) schedule();
    });
    measure();
    mutations.observe(document.body, { subtree: true, childList: true, attributes: true, attributeFilter: ['data-workbench-chrome', 'data-screen-titlebar'] });
    window.addEventListener('resize', schedule);
    return () => {
      observer?.disconnect(); mutations.disconnect();
      if (frame !== null) cancelAnimationFrame(frame);
      window.removeEventListener('resize', schedule);
    };
  }, []);
  return bottom;
}
