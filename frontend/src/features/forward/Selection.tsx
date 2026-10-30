/** Multi-select of chat messages, or of any items of a thread across its turns.
 *
 * Selecting puts a conversation in a mode of its own: a checkbox on every message or item, `Shift+click`
 * for everything between two, and a bar in place of the composer that says how many are picked and
 * what can be done with them: forward them or copy them, or cancel. The rules of the picking are
 * plain functions of an order of ids, so that the same selection serves a chat's messages and a
 * thread's items.
 */
import { Copy, Forward, X } from 'lucide-react';
import { type ReactNode, useCallback, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/Button';
import { Tooltip } from '@/ui/Tooltip';

/** The ids from `from` to `to` in `order`, both included, whichever comes first. */
export function between<Id>(order: readonly Id[], from: Id, to: Id): Id[] {
  const one = order.indexOf(from);
  const other = order.indexOf(to);
  if (one < 0 || other < 0) return [to];
  return order.slice(Math.min(one, other), Math.max(one, other) + 1);
}

export interface Selection<Id> {
  /** Whether messages are being picked. */
  active: boolean;
  /** Everything picked, in the order of the conversation. */
  ids: readonly Id[];
  has: (id: Id) => boolean;
  /** Picks or lets go of one; with `range`, of everything between the last one touched and this. */
  toggle: (id: Id, range: boolean) => void;
  /** Begins picking, with one already picked. */
  start: (id: Id) => void;
  /** Picks these too, beginning to pick where nothing was: a whole turn at once. */
  pick: (ids: readonly Id[]) => void;
  clear: () => void;
}

export function useSelection<Id extends string>(order: readonly Id[]): Selection<Id> {
  const [picked, setPicked] = useState<ReadonlySet<Id> | null>(null);
  const [last, setLast] = useState<Id | null>(null);

  const toggle = useCallback(
    (id: Id, range: boolean) => {
      setPicked((before) => {
        const next = new Set(before ?? []);
        if (range && last !== null) {
          const on = !(before?.has(id) ?? false);
          for (const between_ of between(order, last, id)) {
            if (on) next.add(between_);
            else next.delete(between_);
          }
        } else if (next.has(id)) next.delete(id);
        else next.add(id);
        return next;
      });
      setLast(id);
    },
    [last, order],
  );

  return useMemo(
    () => ({
      active: picked !== null,
      ids: order.filter((id) => picked?.has(id) ?? false),
      has: (id) => picked?.has(id) ?? false,
      toggle,
      start: (id) => {
        setPicked(new Set([id]));
        setLast(id);
      },
      pick: (ids) => {
        if (ids.length === 0) return;
        setPicked((before) => new Set([...(before ?? []), ...ids]));
        setLast(ids[ids.length - 1] ?? null);
      },
      clear: () => {
        setPicked(null);
        setLast(null);
      },
    }),
    [picked, order, toggle],
  );
}

export interface SelectionBarProps {
  count: number;
  /** The most that can be forwarded at once; more picked is allowed, and the button then says why it waits. */
  most: number;
  onForward: () => void;
  onCopy: () => void;
  onCancel: () => void;
  /** What the bar is called, for a screen reader: by default it is the messages of a chat that are picked. */
  label?: string;
  /** What is said when more are picked than can be forwarded: by default it is messages. */
  tooManyText?: string;
  /** More acts on what is picked, between Forward and Copy. */
  extra?: ReactNode;
}

export function SelectionBar({ count, most, onForward, onCopy, onCancel, label, tooManyText, extra }: SelectionBarProps) {
  const { t } = useTranslation();
  const tooMany = count > most;
  return (
    <div role="toolbar" aria-label={label ?? t('forward.bar.label')} className="flex flex-wrap items-center gap-2 border-t border-border bg-bg-subtle px-4 py-3">
      <span className="text-sm font-medium text-text" aria-live="polite">
        {t('forward.bar.selected', { count })}
      </span>
      <span className="flex-1" />
      <Tooltip content={tooMany ? (tooManyText ?? t('forward.bar.tooMany', { count: most })) : undefined}>
        <Button size="sm" variant="primary" icon={<Forward aria-hidden />} blocked={tooMany || count === 0} onClick={onForward}>
          {t('forward.bar.forward')}
        </Button>
      </Tooltip>
      {extra}
      <Button size="sm" icon={<Copy aria-hidden />} disabled={count === 0} onClick={onCopy}>
        {t('forward.bar.copy')}
      </Button>
      <Button size="sm" variant="ghost" icon={<X aria-hidden />} onClick={onCancel}>
        {t('common.cancel')}
      </Button>
    </div>
  );
}
