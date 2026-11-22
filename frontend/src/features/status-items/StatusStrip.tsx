/** First-fitting status items stay on the line; the same values remain reachable in its overflow card. */
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { Ellipsis } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { IconButton } from '@/ui/IconButton';
import { Popover } from '@/ui/Popover';
import { cx } from '@/ui/cx';

export interface StatusSlot { id: string; content: ReactNode; text?: string }
/** The shared small icon control is 28px; reserve it and one ordinary 4px item gap. */
const OVERFLOW_WIDTH = 32;
const ITEM_GAP = 4;
export function StatusStrip({ items, rows = 1 }: { items: readonly StatusSlot[]; rows?: 1 | 2 }) {
  const { t } = useTranslation();
  const at = useRef<HTMLDivElement>(null);
  const [fit, setFit] = useState<number | null>(null);
  const count = fit ?? items.length;
  const order = JSON.stringify(items.map((item) => item.id));
  useLayoutEffect(() => {
    const strip = at.current;
    if (strip === null) return;
    const slots = Array.from(strip.querySelectorAll<HTMLElement>(':scope > [data-status-item]'));
    // A returning strip keeps its count until the browser delivers the new slot sizes.
    const widths = new Map<Element, number>();
    const observer = new ResizeObserver((entries) => {
      for (const entry of entries) widths.set(entry.target, entry.contentRect.width);
      const available = widths.get(strip);
      if (available === undefined) return;
      if (available <= 0) { setFit(null); return; }
      let line = 1, used = 0, withMore = 0;
      for (const [index, slot] of slots.entries()) {
        const width = Math.min(available, widths.get(slot) ?? 0);
        if (width > 0) {
          if (used > 0 && used + ITEM_GAP + width > available) { line++; used = 0; }
          used += (used > 0 ? ITEM_GAP : 0) + width;
        }
        if (line > rows) { setFit(withMore); return; }
        if (line < rows || used === 0 || used + ITEM_GAP + OVERFLOW_WIDTH <= available) withMore = index + 1;
      }
      setFit(null);
    });
    observer.observe(strip);
    for (const slot of slots) observer.observe(slot);
    return () => observer.disconnect();
  }, [order, rows]);
  const rest = items.slice(count);
  const firstText = items.findIndex((item) => item.text !== undefined);
  return <div ref={at} className={cx('relative flex min-w-0 flex-1 items-center gap-1', rows === 2 && 'flex-wrap justify-center')} data-status-strip>
    {items.map((item, index) => <span key={item.id}
      aria-hidden={index >= count || undefined} inert={index >= count} data-status-item={item.id}
      className={cx('inline-flex max-w-full shrink-0 items-center empty:hidden', index >= count && 'pointer-events-none invisible absolute top-0')}>
      {rows === 2 && firstText >= 0 && firstText < index && <span aria-hidden className="mr-1">{t('resourceFacts.separator')}</span>}{item.content}
    </span>)}
    {rest.length > 0 && <Popover label={t('statusItems.more')} side="top" align="end" trigger={<IconButton size="sm" label={t('statusItems.more')} icon={<Ellipsis aria-hidden />} />}>
      <ul className="flex max-w-72 flex-col gap-2">{rest.map((item) => <li key={item.id} data-status-item={item.id} className="min-w-0">{item.content}</li>)}</ul>
    </Popover>}
  </div>;
}
