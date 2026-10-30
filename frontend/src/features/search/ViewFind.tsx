/** A view's own Find bar keeps its search, highlights and return focus beside the content it searches. */
import { type RefObject, useEffect, useId, useRef, useState } from 'react';

import type { SearchFilters } from '@/api/search';
import { FindBar, useFind } from '@/features/search/Find';
import { clearLight, lightWords } from '@/features/search/highlight';
import { keyOfHit } from '@/features/search/hits';
import { captureFocus, restoreFocus, type ReturnFocus } from '@/features/search/returnFocus';
import type { SearchHit } from '@/generated/api-types';
import { bindAction } from '@/shell/keys';
import type { ThreadReadTarget } from '@/realtime/threadScope';

// One configurable action dispatches to the view that contains the key's target. Opening another
// conversation therefore adds a candidate without replacing the first conversation's binding.
const keyReaders = new Set<(event: KeyboardEvent) => boolean>();
let stopKeys: (() => void) | null = null;

/** Adds a currently drawn view to the one Find action; a key outside it remains the browser's. */
export function useFindShortcut(read: (event: KeyboardEvent) => boolean): void {
  // Keep one registration per view while its current scope and open state follow each render.
  const latest = useRef(read);
  useEffect(() => { latest.current = read; }, [read]);
  useEffect(() => {
    const reader = (event: KeyboardEvent) => latest.current(event);
    keyReaders.add(reader);
    stopKeys ??= bindAction('find', (event) => {
      for (const reader of keyReaders) if (reader(event)) return true;
      return false;
    });
    return () => {
      keyReaders.delete(reader);
      if (keyReaders.size === 0) {
        stopKeys?.();
        stopKeys = null;
      }
    };
  }, []);
}

/** A repaint waits for a burst of virtual rows or editor changes to settle. */
const LIGHT_AFTER_MS = 60;
/** A few hits before the end prefetch the next ordinary search page. */
const PREFETCH_REMAINING = 3;

export interface ViewFindOptions {
  root: RefObject<HTMLElement | null>;
  filters: Omit<SearchFilters, 'q'>;
  label: string;
  onHit: (hit: SearchHit) => void;
  /** Only the content, excluding the field and any unsent draft, is lit. */
  content: string;
  enabled?: boolean;
  historyThread?: ThreadReadTarget;
}

/** Keeps Find in one authorized view, including a second view of a chat, document or conversation. */
export function useViewFind({ root, filters, label, onHit, content, enabled = true, historyThread }: ViewFindOptions) {
  const owner = useId();
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState('');
  const [index, setIndex] = useState(0);
  const focus = useRef<ReturnFocus | null>(null);

  const show = () => {
    if (!enabled || root.current === null) return;
    if (open) {
      root.current.querySelector<HTMLInputElement>('[role="search"] input')?.focus();
      return;
    }
    focus.current = captureFocus(root.current);
    setOpen(true);
  };

  const readKey = (event: KeyboardEvent): boolean => {
    if (!enabled || root.current === null || !(event.target instanceof Node) || !root.current.contains(event.target)) return false;
    show();
    return true;
  };
  useFindShortcut(readKey);

  const found = useFind(filters, typed, enabled && open, historyThread);
  const at = found.hits.length === 0 ? null : Math.min(index, found.hits.length - 1);
  const hit = at === null ? undefined : found.hits[at];
  const key = hit === undefined ? null : keyOfHit(hit);
  const latestHit = useRef(hit);
  useEffect(() => { latestHit.current = hit; }, [hit]);
  useEffect(() => {
    const current = latestHit.current;
    if (key !== null && current !== undefined) onHit(current);
  }, [key, onHit]);

  useEffect(() => {
    const element = root.current;
    if (!enabled || !open || element === null || typed.trim() === '') {
      clearLight(owner);
      return;
    }
    let timer: ReturnType<typeof setTimeout> | undefined;
    const light = () => lightWords(element.querySelector(content), typed, owner);
    light();
    const watch = new MutationObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(light, LIGHT_AFTER_MS);
    });
    watch.observe(element, { childList: true, subtree: true, characterData: true });
    return () => {
      clearTimeout(timer);
      watch.disconnect();
      clearLight(owner);
    };
  }, [enabled, open, typed, root, content, owner]);

  const close = () => {
    setOpen(false);
    clearLight(owner);
    const target = focus.current;
    queueMicrotask(() => restoreFocus(target));
  };

  const step = (direction: 'older' | 'newer') => {
    if (at === null) return;
    if (direction === 'newer') setIndex(at === 0 ? found.hits.length - 1 : at - 1);
    else if (at + 1 < found.hits.length) {
      setIndex(at + 1);
      if (found.more && at + PREFETCH_REMAINING >= found.hits.length) found.fetchMore();
    } else if (found.more) {
      setIndex(at + 1);
      found.fetchMore();
    } else setIndex(0);
  };

  return {
    open: enabled && open,
    show,
    bar: enabled && open ? <FindBar label={label} typed={typed} onTyped={(next) => { setTyped(next); setIndex(0); }} at={at} found={found} onStep={step} onClose={close} /> : null,
  };
}
