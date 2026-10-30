/** Two visible Find bars keep their own scope, paging, highlights and return caret while Enter never sends a draft. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useCallback, useRef, useState } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as Search from '@/api/search';
import type { SearchFilters } from '@/api/search';
import { useViewFind } from '@/features/search/ViewFind';
import type { SearchHit, SearchHitPage } from '@/generated/api-types';
import { SHIPPED_CHOICES } from '@/shell/keyActions';
import { keyBook, setKeyChoices } from '@/shell/keys';
import { chat, message } from '../doubles/chat-rows';
import { show } from './support';

const requests = vi.hoisted(() => vi.fn<(filters: SearchFilters, cursor: string | null) => void>());
vi.mock('@/api/search', async (original) => {
  const source = await original<typeof Search>();
  return { ...source, searchQuery: (filters: SearchFilters) => ({
    queryKey: source.searchKey(filters), initialPageParam: null as string | null,
    queryFn: ({ pageParam }: { pageParam: string | null }): Promise<SearchHitPage> => {
      requests(filters, pageParam);
      const id = filters.chat_id ?? '';
      const older = pageParam !== null;
      const hit: SearchHit = { kind: 'chat_message', chat: chat({ id }), message: message(older ? 1 : 2, { id: `${id}-${older ? 'older' : 'newer'}`, chat_id: id }), path: [], snippet: older ? 'hi older' : 'hi newer', matches: [[0, 2]] };
      return Promise.resolve({ items: filters.q === 'hi' ? [hit] : [], next_cursor: older || filters.q !== 'hi' ? null : 'older' });
    },
    getNextPageParam: (page: SearchHitPage) => page.next_cursor ?? undefined,
  }) };
});

class LitWords {
  ranges: Range[];
  constructor(...ranges: Range[]) { this.ranges = ranges; }
}
const highlights = new Map<string, LitWords>();

function View({ name }: { name: string }) {
  const root = useRef<HTMLElement>(null);
  const [picked, setPicked] = useState('');
  const [sent, setSent] = useState(0);
  const onHit = useCallback((hit: SearchHit) => { if (hit.kind === 'chat_message') setPicked(hit.message.id); }, []);
  const find = useViewFind({ root, filters: { chat_id: name, kinds: ['chat_message'] }, label: `Find ${name}`, content: '[data-find-content]', onHit });
  return <section ref={root} aria-label={name} onKeyDown={(event) => { if (event.key === 'Enter') setSent((count) => count + 1); }}>
    <button type="button" onClick={find.show}>{`Find ${name}`}</button>
    {find.bar}
    <div data-find-content><p>hi newer</p><p>hi older</p></div>
    <output aria-label={`${name} match`}>{picked}</output>
    <output aria-label={`${name} sent`}>{sent}</output>
    <textarea aria-label={`${name} draft`} defaultValue="Keep this unsent" />
  </section>;
}

beforeEach(() => {
  requests.mockClear(); highlights.clear();
  vi.stubGlobal('CSS', { highlights }); vi.stubGlobal('Highlight', LitWords);
  setKeyChoices(SHIPPED_CHOICES);
});
afterEach(() => { cleanup(); setKeyChoices(SHIPPED_CHOICES); vi.unstubAllGlobals(); });

function pair() {
  return show(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    <div onKeyDown={(event) => { keyBook.handle(event.nativeEvent); }}><View name="left" /><View name="right" /></div>
  </QueryClientProvider>);
}

it('steps into a further page in one chat and closes only its own highlights, giving its draft the same caret back', async () => {
  pair();
  const user = userEvent.setup();
  const left = within(screen.getByRole('region', { name: 'left' }));
  const right = within(screen.getByRole('region', { name: 'right' }));
  const draft = left.getByRole('textbox', { name: 'left draft' });
  await user.click(draft);
  if (!(draft instanceof HTMLTextAreaElement)) throw new Error('the draft is a textarea');
  draft.setSelectionRange(3, 3);
  await user.keyboard('{Control>}f{/Control}');
  const leftBar = left.getByRole('search', { name: 'Find left' });
  const leftField = within(leftBar).getByRole('textbox');
  expect(leftField).toHaveFocus();
  await user.type(leftField, 'hi');
  await waitFor(() => expect(within(leftBar).getByRole('status')).toHaveTextContent('1 of 1+'));
  await user.keyboard('{Enter}');
  await waitFor(() => expect(within(leftBar).getByRole('status')).toHaveTextContent('2 of 2'));
  expect(left.getByLabelText('left match')).toHaveTextContent('left-older');
  expect(left.getByLabelText('left sent')).toHaveTextContent('0');
  expect(requests.mock.calls.some(([filters, cursor]) => filters.chat_id === 'left' && cursor === 'older')).toBe(true);

  await user.click(right.getByRole('textbox', { name: 'right draft' }));
  await user.keyboard('{Control>}f{/Control}');
  const rightBar = right.getByRole('search', { name: 'Find right' });
  await user.type(within(rightBar).getByRole('textbox'), 'hi');
  await waitFor(() => expect(within(rightBar).getByRole('status')).toHaveTextContent('1 of 1+'));
  await waitFor(() => expect(highlights.get('orch-find')?.ranges).toHaveLength(4));
  await user.click(leftField);
  await user.keyboard('{Escape}');
  expect(left.queryByRole('search')).toBeNull();
  expect(draft).toHaveFocus();
  expect(draft).toHaveValue('Keep this unsent');
  expect(draft.selectionStart).toBe(3);
  expect(rightBar).toBeVisible();
  await waitFor(() => expect(highlights.get('orch-find')?.ranges).toHaveLength(2));
});

it('uses the person’s chosen Find chord in whichever view contains the focus', async () => {
  pair();
  setKeyChoices({ single_key: true, bindings: { find: 'Mod+Shift+F' } });
  const user = userEvent.setup();
  await user.click(screen.getByRole('textbox', { name: 'right draft' }));
  await user.keyboard('{Control>}f{/Control}');
  expect(screen.queryByRole('search')).toBeNull();
  await user.keyboard('{Control>}{Shift>}f{/Shift}{/Control}');
  expect(screen.getByRole('search', { name: 'Find right' })).toBeVisible();
  expect(screen.queryByRole('search', { name: 'Find left' })).toBeNull();
});
