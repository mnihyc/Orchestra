/** Finding words inside one conversation, chat or document: the bar with its field, its count and its two arrows, and the hits behind it.
 *
 * The same search as the page of search, kept to one place (`scope`), with its hits read a page at a time as
 * the reader steps through them. The bar is drawn by whatever screen holds the conversation; this part knows
 * neither the screen nor how it brings an item into view.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { ArrowDown, ArrowUp, Search, X } from 'lucide-react';
import { type KeyboardEvent, useEffect, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { ITEM_KINDS, searchQuery, tooShort, type SearchFilters } from '@/api/search';
import { Hint } from '@/features/help/Hint';
import { keyOfHit } from '@/features/search/hits';
import type { SearchHit } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import { useThreadReadScope } from '@/realtime/useThreadRead';
import type { ThreadReadTarget } from '@/realtime/threadScope';
import { IconButton } from '@/ui/IconButton';
import { fieldBoxClasses } from '@/ui/Input';

/** How many hits a page of a find holds: stepping through them goes on for a while before another page is wanted. */
const FIND_PAGE = 50;
/** How long the field rests before the words are looked for: the keys of one word are one search. */
const FIND_PAUSE_MS = 250;

export interface Found {
  hits: SearchHit[];
  /** More hits than the ones read so far. */
  more: boolean;
  fetchMore: () => void;
  pending: boolean;
  failed: boolean;
  /** The words are too short to look for. */
  short: boolean;
}

/** The hits of words inside one place, newest first, once the field has rested. */
export function useFind(scope: string | Omit<SearchFilters, 'q'>, typed: string, enabled = true, historyThread?: ThreadReadTarget): Found {
  useThreadReadScope(historyThread ?? null);
  const q = useDebounced(typed.trim(), FIND_PAUSE_MS);
  const short = tooShort(q, { narrow: true }) !== null;
  const filters = typeof scope === 'string' ? { scope, kinds: ITEM_KINDS } : scope;
  const options = searchQuery({ ...filters, q }, FIND_PAGE, historyThread);
  const search = useInfiniteQuery({ ...options, enabled: enabled && !short && options.enabled });
  const hits = useMemo(() => {
    const seen = new Set<string>();
    return (search.data?.pages ?? []).flatMap((page) => page.items).filter((hit) => {
      const key = keyOfHit(hit);
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
  }, [search.data]);
  const { hasNextPage, isFetchingNextPage, fetchNextPage } = search;
  return {
    hits: short || !enabled || typed.trim() !== q ? [] : hits,
    more: hasNextPage,
    fetchMore: () => {
      if (hasNextPage && !isFetchingNextPage) void fetchNextPage();
    },
    pending: !short && (search.isPending || typed.trim() !== q),
    failed: search.isError,
    short,
  };
}

export interface FindBarProps {
  /** What the bar is for, said to a screen reader: "Find in the conversation". */
  label: string;
  typed: string;
  onTyped: (typed: string) => void;
  /** The hit in front, from 0; `null` when there is none. */
  at: number | null;
  found: Found;
  /** Steps to an older hit, or a newer one. */
  onStep: (direction: 'older' | 'newer') => void;
  onClose: () => void;
}

export function FindBar({ label, typed, onTyped, at, found, onStep, onClose }: FindBarProps) {
  const { t } = useTranslation();
  const field = useRef<HTMLInputElement>(null);
  useEffect(() => field.current?.focus(), []);

  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.nativeEvent.isComposing) return;
    if (event.key === 'Enter') {
      event.preventDefault();
      event.stopPropagation();
      onStep(event.shiftKey ? 'newer' : 'older');
    } else if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      onClose();
    }
  };

  const said = found.failed
    ? t('search.find.failed')
    : found.short
      ? typed.trim() === ''
        ? ''
        : t('search.find.short')
      : found.pending
        ? t('search.find.looking')
        : found.hits.length === 0
          ? t('search.find.none')
          : t('search.find.count', { at: (at ?? 0) + 1, total: `${found.hits.length}${found.more ? '+' : ''}` });

  return (
    <div role="search" aria-label={label} className="flex items-center gap-2 border-b border-border bg-bg-subtle px-4 py-2">
      <Search aria-hidden className="text-text-subtle" />
      <input
        ref={field}
        value={typed}
        onChange={(event) => onTyped(event.target.value)}
        onKeyDown={onKeyDown}
        placeholder={label}
        aria-label={label}
        autoComplete="off"
        spellCheck={false}
        className={`${fieldBoxClasses} h-(--control-sm) max-w-(--field-wide) flex-1`}
      />
      <span role="status" className="min-w-0 shrink-0 text-sm text-text-muted">
        {said}
      </span>
      <IconButton size="sm" label={t('search.find.older')} icon={<ArrowUp aria-hidden />} disabled={found.hits.length === 0} onClick={() => onStep('older')} />
      <IconButton size="sm" label={t('search.find.newer')} icon={<ArrowDown aria-hidden />} disabled={found.hits.length === 0} onClick={() => onStep('newer')} />
      <Hint id="find-in-conversation" />
      <IconButton size="sm" label={t('search.find.close')} icon={<X aria-hidden />} onClick={onClose} />
    </div>
  );
}
