/** Shared EmojiPicker component (people); primitives are wrapped here.
 *
 * The platform's own emoji, in groups and searched by name, with no image of its own: a reaction
 * under a message and an emoji in a composer are chosen from here. It is a panel, not a popover: the
 * control that opens it puts it into a `Popover`. The grid is one tab stop; the arrow keys move
 * through it and `Enter` or `Space` takes the emoji in front.
 */
import { Coffee, Gamepad2, Hand, Heart, Hash, Lightbulb, Plane, Search, Smile, SmilePlus, Sprout, type LucideIcon } from 'lucide-react';
import { type KeyboardEvent, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { cx } from '@/ui/cx';
import { EMOJI, EMOJI_GROUPS, type EmojiGroup, emojiName, findEmoji } from '@/ui/emoji-data';
import { IconButton } from '@/ui/IconButton';
import { Popover } from '@/ui/Popover';
import { DEFAULT_REACTIONS, loadReactionHistory, rememberReaction, useReactionHistory } from '@/features/people/reactions';

const GROUP_ICONS: Record<EmojiGroup, LucideIcon> = {
  smileys: Smile,
  gestures: Hand,
  hearts: Heart,
  nature: Sprout,
  food: Coffee,
  activity: Gamepad2,
  travel: Plane,
  objects: Lightbulb,
  symbols: Hash,
};

/** How many emoji stand in a row: with a cell of two rem, it fills the width of a popover of the usual size. */
const COLUMNS = 8;

export interface EmojiPickerProps {
  /** Told which emoji was taken. The picker does not close itself: whoever opened it does. */
  onPick: (emoji: string) => void;
  className?: string;
}

export function EmojiPicker({ onPick, className }: EmojiPickerProps) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const [group, setGroup] = useState<EmojiGroup>('smileys');
  const [at, setAt] = useState(0);
  const cells = useRef<(HTMLButtonElement | null)[]>([]);
  const searching = query.trim() !== '';
  const recent = useReactionHistory((state) => state.recent);
  const often = recent.length === 0 ? DEFAULT_REACTIONS : recent;
  useEffect(() => { void loadReactionHistory().catch(() => undefined); }, []);
  const choose = (emoji: string) => { rememberReaction(emoji); onPick(emoji); };
  const entries = searching ? findEmoji(query) : EMOJI[group];
  const active = Math.min(at, Math.max(0, entries.length - 1));

  const move = (to: number) => {
    const next = Math.max(0, Math.min(entries.length - 1, to));
    setAt(next);
    cells.current[next]?.focus();
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step: Record<string, number> = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: COLUMNS, ArrowUp: -COLUMNS };
    const by = step[event.key];
    if (by !== undefined) {
      event.preventDefault();
      move(active + by);
    } else if (event.key === 'Home') {
      event.preventDefault();
      move(0);
    } else if (event.key === 'End') {
      event.preventDefault();
      move(entries.length - 1);
    }
  };

  return (
    <div className={cx('flex min-h-0 w-(--menu-max-width) max-w-full flex-col gap-2', className)}>
      <label className="relative block">
        <span className="sr-only">{t('chat.emoji.search')}</span>
        <Search aria-hidden className="pointer-events-none absolute top-1/2 left-2 -translate-y-1/2 text-text-subtle" size={14} />
        <input
          type="search"
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setAt(0);
          }}
          placeholder={t('chat.emoji.search')}
          className="h-(--control-sm) w-full rounded-md border border-border-strong bg-surface pr-2 pl-8 text-base text-text placeholder:text-text-subtle"
        />
      </label>
      {!searching && <div><p className="mb-1 text-xs text-text-muted">{t('people.recentReactions')}</p><div role="group" aria-label={t('people.recentReactions')} className="grid grid-cols-8 gap-0.5">{often.map((emoji) => { const entry = Object.values(EMOJI).flat().find((entry) => entry[0] === emoji); return <button key={emoji} type="button" aria-label={entry === undefined ? emoji : emojiName(entry)} onClick={() => choose(emoji)} className="focus-inset inline-flex size-8 items-center justify-center rounded-md text-xl hover:bg-bg-muted">{emoji}</button>; })}</div></div>}
      {!searching && (
        <div role="group" aria-label={t('chat.emoji.groups')} className="flex items-center justify-between gap-0.5">
          {EMOJI_GROUPS.map((name) => {
            const Icon = GROUP_ICONS[name];
            return (
              <button
                key={name}
                type="button"
                aria-label={t(`chat.emoji.group.${name}`)}
                aria-pressed={group === name}
                onClick={() => {
                  setGroup(name);
                  setAt(0);
                }}
                className={cx(
                  'inline-flex size-8 items-center justify-center rounded-md text-text-muted transition-colors hover:bg-bg-muted hover:text-text',
                  group === name && 'bg-bg-muted text-text',
                )}
              >
                <Icon size={16} aria-hidden />
              </button>
            );
          })}
        </div>
      )}
      {entries.length === 0 ? (
        <EmptyState compact className="px-2" title={t('chat.emoji.none')} />
      ) : (
        <div
          role="group"
          aria-label={searching ? t('chat.emoji.found') : t(`chat.emoji.group.${group}`)}
          onKeyDown={onKeyDown}
          className="grid min-h-0 max-h-[30dvh] grid-cols-8 gap-0.5 overflow-y-auto"
        >
          {entries.map((entry, index) => (
            <button
              key={entry[0]}
              ref={(element) => {
                cells.current[index] = element;
              }}
              type="button"
              tabIndex={index === active ? 0 : -1}
              aria-label={emojiName(entry)}
              onClick={() => choose(entry[0])}
              className="focus-inset inline-flex size-8 items-center justify-center rounded-md text-xl transition-colors hover:bg-bg-muted"
            >
              <span aria-hidden>{entry[0]}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** The button that opens the emoji to react with, under a message or a comment: the emoji taken is given back and the picker put away. */
export function ReactButton({ onPick, disabled }: { onPick: (emoji: string) => void; disabled?: boolean }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return (
    <Popover
      label={t('chat.react')}
      scroll="children"
      side="top"
      align="end"
      open={open}
      onOpenChange={setOpen}
      trigger={<IconButton size="sm" label={t('chat.react')} icon={<SmilePlus aria-hidden />} disabled={disabled ?? false} />}
    >
      <EmojiPicker
        onPick={(emoji) => {
          // One reaction at a time: the picker is put away, and the reaction shows at once.
          setOpen(false);
          onPick(emoji);
        }}
      />
    </Popover>
  );
}
