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
  const at = useRef<HTMLDivElement>(null), widths = useRef(new Map<string, HTMLSpanElement>());
  const [count, setCount] = useState(items.length);
  const signature = items.map((item) => `${item.id}:${item.text ?? ''}`).join('|');
  useLayoutEffect(() => {
    const measure = () => {
      const available = at.current?.getBoundingClientRect().width ?? 0;
      if (available <= 0) { setCount(items.length); return; }
      const sizes = items.map((item) => Math.min(available, widths.current.get(item.id)?.scrollWidth ?? 0));
      const fits = (n: number, overflow: boolean) => {
        let line = 1, used = 0;
        for (const width of [...sizes.slice(0, n), ...(overflow ? [OVERFLOW_WIDTH] : [])]) {
          if (width === 0) continue;
          if (used > 0 && used + ITEM_GAP + width > available) { line += 1; used = 0; }
          used += (used > 0 ? ITEM_GAP : 0) + width;
        }
        return line <= rows;
      };
      let next = items.length;
      if (!fits(next, false)) { while (next > 0 && !fits(next, true)) next -= 1; }
      setCount(next);
    };
    measure();
    const observer = new ResizeObserver(measure);
    if (at.current !== null) observer.observe(at.current);
    for (const element of widths.current.values()) observer.observe(element);
    return () => observer.disconnect();
  }, [items, rows, signature]);
  const rest = items.slice(count);
  return <div ref={at} className={cx('relative flex min-w-0 flex-1 items-center gap-1', rows === 2 && 'flex-wrap justify-center')} data-status-strip>
    {items.map((item, index) => <span key={item.id} ref={(element) => { if (element === null) widths.current.delete(item.id); else widths.current.set(item.id, element); }}
      aria-hidden={index >= count || undefined} inert={index >= count} data-status-item={item.id}
      className={cx('inline-flex max-w-full shrink-0 items-center empty:hidden', index >= count && 'pointer-events-none invisible absolute top-0')}>
      {rows === 2 && items.slice(0, index).some((before) => before.text !== undefined) && <span aria-hidden className="mr-1">{t('resourceFacts.separator')}</span>}{item.content}
    </span>)}
    {rest.length > 0 && <Popover label={t('statusItems.more')} side="top" align="end" trigger={<IconButton size="sm" label={t('statusItems.more')} icon={<Ellipsis aria-hidden />} />}>
      <ul className="flex max-w-72 flex-col gap-2">{rest.map((item) => <li key={item.id} data-status-item={item.id} className="min-w-0">{item.content}</li>)}</ul>
    </Popover>}
  </div>;
}
