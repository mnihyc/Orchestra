/** The title keeps its natural room before quick and primary header actions fold into the existing menu. */
import { type RefObject, useLayoutEffect, useState } from 'react';

type Actions = 'all' | 'primary' | 'menu';
interface Layout { actions: Actions; stacked: boolean }

/** A display:contents wrapper contributes its actual flex children, including shared guide triggers. */
function items(parent: Element): HTMLElement[] {
  return Array.from(parent.children).flatMap((child) => {
    if (!(child instanceof HTMLElement)) return [];
    const display = getComputedStyle(child).display;
    if (display === 'none') return [];
    if (display === 'contents') return items(child);
    return child.getBoundingClientRect().width > 0 ? [child] : [];
  });
}
const gap = (element: HTMLElement) => Number.parseFloat(getComputedStyle(element).columnGap) || 0;

export function useTitlePriority(ref: RefObject<HTMLElement | null>, compact: boolean, name: string, actionsKey: string) {
  const [layout, setLayout] = useState<Layout>({ actions: 'all', stacked: false });
  useLayoutEffect(() => {
    const header = ref.current;
    if (header === null) return;
    const row = header.querySelector<HTMLElement>('[data-thread-title-row]');
    const identity = header.querySelector<HTMLElement>('[data-thread-identity]');
    const quick = header.querySelector<HTMLElement>('[data-thread-quick-actions]');
    const primary = header.querySelector<HTMLElement>('[data-thread-primary-actions]');
    const actions = header.querySelector<HTMLElement>('[data-thread-actions-row]');
    const more = header.querySelector<HTMLElement>('[data-thread-more]');
    if (row === null || identity === null || quick === null || primary === null || actions === null || more === null) return;
    let active = true;
    const measure = () => {
      if (!active) return;
      const heading = identity.querySelector('h1');
      const width = row.getBoundingClientRect().width;
      // A live edit keeps the already chosen layout. Layoutless renderers keep their ordinary controls.
      if (heading === null || width <= 0) return;
      const control = heading.closest('button') ?? heading;
      const text = document.createRange(); text.selectNodeContents(heading);
      const title = text.getBoundingClientRect().width + Math.max(0, control.getBoundingClientRect().width - heading.getBoundingClientRect().width);
      const fixed = items(identity).filter((item) => !item.contains(heading));
      const natural = title + fixed.reduce((sum, item) => sum + item.getBoundingClientRect().width, 0) + fixed.length * gap(identity);
      const base = natural + more.getBoundingClientRect().width + gap(row);
      const primaryWidth = primary.getBoundingClientRect().width;
      const quickWidth = quick.getBoundingClientRect().width;
      const actionGap = gap(actions);
      const next: Layout = compact ? { actions: 'all', stacked: true }
        : base + primaryWidth + quickWidth + 2 * actionGap <= width ? { actions: 'all', stacked: false }
        : base + primaryWidth + actionGap <= width ? { actions: 'primary', stacked: false }
        : { actions: 'menu', stacked: base > width };
      const focused = document.activeElement;
      if (next.actions !== 'all' && quick.contains(focused) || next.actions === 'menu' && primary.contains(focused)) more.focus({ preventScroll: true });
      setLayout((held) => held.actions === next.actions && held.stacked === next.stacked ? held : next);
    };
    measure();
    const observer = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(measure);
    for (const element of [row, identity, quick, primary, more, ...items(identity)]) observer?.observe(element);
    window.addEventListener('resize', measure);
    if (document.fonts !== undefined) void document.fonts.ready.then(measure);
    return () => { active = false; observer?.disconnect(); window.removeEventListener('resize', measure); };
  }, [ref, compact, name, actionsKey]);
  return layout;
}
