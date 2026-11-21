/** Typed searches belong to one collection and one requesting reader. */
import { useState } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { useDebounced } from '@/lib/debounce';

export function useCollectionSearch(id: string) {
  const reader = useStore(client.session, (state) => String(state.userId) + ':' + String(state.readerVersion));
  const scope = reader + ':' + id;
  const [draft, setDraft] = useState({ scope, text: '' });
  const text = draft.scope === scope ? draft.text : '';
  const prefix = scope + '\u0000';
  const delayed = useDebounced(prefix + text.trim());
  const q = delayed.startsWith(prefix) ? delayed.slice(prefix.length) : '';
  return { scope, text, q, onText: (next: string) => setDraft({ scope, text: next }) };
}

/** Match the native literal-word grammar while joining held live rows to a searched history page. */
export function matchesCollectionWords(query: string, text: string): boolean {
  const terms: string[] = [];
  let term = '', quoted = false;
  for (const character of query) {
    if (character === '"') quoted = !quoted;
    else if (/\s/u.test(character) && !quoted) {
      if (term.trim() !== '') terms.push(term.trim());
      term = '';
    } else term += character;
  }
  if (term.trim() !== '') terms.push(term.trim());
  const folded = text.toLocaleLowerCase();
  return terms.every((word) => folded.includes(word.toLocaleLowerCase()));
}
