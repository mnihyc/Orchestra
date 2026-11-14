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
  return { text, q, onText: (next: string) => setDraft({ scope, text: next }) };
}
