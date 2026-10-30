/** Native links and specialized tabs remain visible, wrapping or stacking with the available room. */
import { type ReactNode, useRef } from 'react';

import { cx } from '@/ui/cx';

export interface FoldingItem {
  id: string;
  label: string;
  content: ReactNode;
  /** Sizing metadata retained for callers; wrapping does not render a second copy of the control. */
  measure?: ReactNode;
  onSelect: () => void;
}

export function FoldingStrip({ label, active, items, role = 'navigation', stackAtWide = false, actions, className }: {
  label: string; active: string; items: readonly FoldingItem[]; role?: 'navigation' | 'tablist' | 'group';
  stackAtWide?: boolean; actions?: ReactNode; className?: string;
}) {
  const outer = useRef<HTMLDivElement>(null);
  const List = role === 'navigation' ? 'ul' : 'div';
  const Item = role === 'navigation' ? 'li' : 'div';
  const choose = (item: FoldingItem) => {
    const control = Array.from(outer.current?.querySelectorAll<HTMLElement>('[data-fold-key]') ?? []).find((node) => node.dataset.foldKey === item.id)?.querySelector<HTMLElement>('a[href],button,[role="tab"]');
    // Native links are followed with Enter; arrow keys only move their focus, including across lines.
    if (control?.matches('a[href]')) { control.focus(); return; }
    item.onSelect();
    requestAnimationFrame(() => Array.from(outer.current?.querySelectorAll<HTMLElement>('[data-fold-key]') ?? []).find((node) => node.dataset.foldKey === item.id)?.querySelector<HTMLElement>('a[href],button,[role="tab"]')?.focus());
  };
  return <div ref={outer} role={role === 'tablist' ? undefined : role} aria-label={role === 'tablist' ? undefined : label} data-active-item={active} className={cx('min-w-0', stackAtWide && '@min-[48rem]:w-[13rem] @min-[48rem]:shrink-0', className)} onKeyDown={(event) => {
    if (event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey) return;
    const id = (event.target as HTMLElement).closest<HTMLElement>('[data-fold-key]')?.dataset.foldKey;
    const at = items.findIndex((item) => item.id === id);
    if (at < 0) return;
    const index = event.key === 'Home' ? 0 : event.key === 'End' ? items.length - 1 : event.key === 'ArrowRight' || event.key === 'ArrowDown' ? (at + 1) % items.length : event.key === 'ArrowLeft' || event.key === 'ArrowUp' ? (at - 1 + items.length) % items.length : null;
    const item = index === null ? undefined : items[index];
    if (item !== undefined) { event.preventDefault(); choose(item); }
  }}>
    <List data-navigation-strip="links" role={role === 'tablist' ? 'tablist' : undefined} aria-label={role === 'tablist' ? label : undefined} className={cx('flex min-w-0 flex-wrap items-stretch gap-y-1', stackAtWide && '@min-[48rem]:flex-col')}>
      {items.map((item) => <Item key={item.id} data-fold-key={item.id} className="flex min-w-0 max-w-full shrink-0 items-stretch pr-1">{item.content}</Item>)}
      {actions !== undefined && <Item className="flex shrink-0 items-start">{actions}</Item>}
    </List>
  </div>;
}
