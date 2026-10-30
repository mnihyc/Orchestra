/** Passive states take one row above the composer, most urgent first. The shared Popover holds the remaining
 * states in their existing forms and owns placement, scrolling, dismissal and focus. Approvals, plan and queue keep
 * their own rows. The primary row stays in place while the remaining states open beside its chip.
 */
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';
import { Popover } from '@/ui/Popover';

export interface PassiveEntry {
  /** What it is, for the row's own bookkeeping. */
  id: string;
  /** The component that draws it, or nothing when it has nothing to say. */
  node: ReactNode;
}

export function PassiveStrip({ entries }: { entries: readonly PassiveEntry[] }) {
  const { t } = useTranslation();
  const wrappers = useRef(new Map<string, HTMLDivElement>());
  const [{ live, open }, setPresence] = useState<{ live: readonly string[]; open: boolean }>({ live: [], open: false });
  const more = Math.max(0, live.length - 1);
  const expanded = open && more > 0;

  // A component that draws nothing leaves its wrapper empty, in the normal row or the portal.
  useLayoutEffect(() => {
    const measure = () => {
      setPresence((held) => {
        // A portal mounts after the normal rows detach. Preserve presence across that handover; a mounted empty
        // wrapper still removes a state immediately when its component has nothing left to show.
        const found = entries.filter((entry) => {
          const wrapper = wrappers.current.get(entry.id);
          return wrapper === undefined ? held.live.includes(entry.id) : wrapper.childElementCount > 0;
        }).map((entry) => entry.id);
        const stillOpen = held.open && found.length > 1;
        return held.open === stillOpen && held.live.length === found.length && held.live.every((id, index) => id === found[index])
          ? held : { live: found, open: stillOpen };
      });
    };
    measure();
    const observer = new MutationObserver(measure);
    for (const wrapper of wrappers.current.values()) observer.observe(wrapper, { childList: true, subtree: true });
    return () => observer.disconnect();
  }, [entries, expanded]);

  const rows = (shown: readonly PassiveEntry[], hideOthers: boolean) => shown.map((entry) => (
    <div key={entry.id} data-passive={entry.id}
      ref={(node) => { if (node === null) wrappers.current.delete(entry.id); else wrappers.current.set(entry.id, node); }}
      className={cx('min-w-0 empty:hidden', hideOthers && live[0] !== undefined && live[0] !== entry.id && 'hidden')}>
      {entry.node}
    </div>
  ));
  return (
    <div className={cx('flex min-w-0 items-start gap-2', live.length === 0 && 'hidden')} data-passive-strip>
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        {rows(expanded ? entries.filter((entry) => entry.id === live[0]) : entries, true)}
      </div>
      {more > 0 && (
        <Popover label={t('thread.passive.more', { count: more })} side="top" align="end" open={expanded}
          onOpenChange={(next) => setPresence((held) => held.open === next ? held : { ...held, open: next })}
          className="w-[min(20rem,calc(100vw-1rem))] max-h-[min(20rem,50dvh)] p-2"
          trigger={<button type="button" aria-label={t('thread.passive.more', { count: more })}
            className="focus-inset inline-flex min-h-8 shrink-0 items-center rounded-full border border-border bg-surface px-2 text-xs text-text-muted tabular-nums hover:bg-bg-muted hover:text-text">+{more}</button>}>
          {expanded ? <div className="flex min-w-0 flex-col gap-2">{rows(entries.filter((entry) => entry.id !== live[0]), false)}</div> : null}
        </Popover>
      )}
    </div>
  );
}
