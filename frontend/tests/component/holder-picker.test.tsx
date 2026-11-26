import { CandidatePicker } from '@/features/access/CandidatePicker';
/** A locator resolves to an accurate, placed holder; typing or loading alone never gives it. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { type Candidate, useCandidates } from '@/features/access/candidates';
import type { HolderSearchReply } from '@/generated/api-types';
import { client } from '@/api/client';
import type * as Recents from '@/api/pickerRecents';
import { i18n, setLocale } from '@/i18n';

import { show } from './support';
import { me, userRef } from '../doubles/fixtures';

const server = vi.hoisted(() => ({ pages: false, read: vi.fn<(q: string, cursor: string | null) => void>(), recent: [] as string[], record: vi.fn<typeof Recents.recordPickerChoice>(), slow: false, release: null as ((reply: HolderSearchReply) => void) | null }));
const dep2 = '00000000-0000-4000-8000-000000000002';
const resolved: HolderSearchReply = { items: [{ input: { kind: 'role', role: 'developer', at: dep2 },
  holder: { kind: 'role', role: 'developer', at: { id: dep2, kind: 'org', name: 'Acme > dep2', redacted: false }, place: 'Acme > dep2', count: 1 },
  locator: 'acme.dep2 developer', place: 'Acme > dep2', outside: false }] };
const zoe = userRef(me(8, 'zoe', 'Zoe Outsider'));
const outsider: HolderSearchReply = { items: [{ input: { kind: 'user', id: zoe.id }, holder: { kind: 'user', user: zoe },
  locator: 'zoe', place: '@zoe', outside: true }] };

const zelda = userRef(me(9, 'zelda', 'Zelda Example')), zipporah = userRef(me(10, 'zipporah', 'Zipporah Example'));
const personRow = (user: typeof zoe): HolderSearchReply['items'][number] => ({ input: { kind: 'user', id: user.id }, holder: { kind: 'user', user }, locator: user.username, place: `@${user.username}`, outside: true });
vi.mock('@/api/access', async (original) => ({ ...await original<object>(),
  holderPickerQuery: (q: string) => ({ queryKey: ['holder-picker-proof', q], initialPageParam: null as string | null,
    getNextPageParam: (page: HolderSearchReply) => page.next_cursor ?? undefined,
    queryFn: ({ pageParam }: { pageParam: string | null }): Promise<HolderSearchReply> => {
    server.read(q, pageParam);
    if (server.pages && (q === '' || q === 'z')) return Promise.resolve({ items: pageParam === null ? [personRow(zelda), personRow(zoe)] : [personRow(zipporah)], ...(pageParam === null ? { next_cursor: 'more' } : {}) });
    if (q === 'zoe') return Promise.resolve(outsider);
    if (q !== 'dep2 developer') return Promise.resolve({ items: [] });
    if (server.slow) return new Promise((resolve) => { server.release = resolve; });
    return Promise.resolve(resolved);
  } }),
}));

vi.mock('@/api/pickerRecents', async (load) => {
  const actual = await load<typeof Recents>();
  return { ...actual, pickerRecentsQuery: (person: string) => ({ ...actual.pickerRecentsQuery(person), queryFn: () => Promise.resolve({ count: 5, choices: { user: server.recent } }) }), recordPickerChoice: server.record };
});
const session = client.session.getState();
afterEach(async () => { cleanup(); client.session.setState(session); await setLocale('en'); });
function Picker({ outsiderOff }: { outsiderOff?: string | undefined }) {
  const [query, setQuery] = useState('');
  const [chosen, setChosen] = useState<Candidate | null>(null);
  const found = useCandidates(query, null, { kinds: ['user', 'group', 'role', 'members'], thing: 'dep3', placeId: dep2, outsiderOff });
  return <>
    <CandidatePicker candidates={found} label="Add members" query={query} onQueryChange={setQuery} groups={found.groups} loading={found.loading}
      empty="Nothing is called that here" onSelect={(option) => {
        const holder = found.byValue.get(option.value);
        if (holder !== undefined) { setChosen(holder); setQuery(holder.name); }
      }} />
    {chosen !== null && <p role="status">Chosen: {chosen.name}</p>}
  </>;
}

function draw(outsiderOff?: string) {
  return show(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><Picker outsiderOff={outsiderOff} /></QueryClientProvider>);
}

beforeEach(() => { server.slow = false; server.release = null; server.pages = false; server.recent = []; server.read.mockClear(); server.record.mockReset().mockResolvedValue({ count: 5, remembered: true }); });

describe('the holder locator', () => {
  it('marks an outsider before they are selected', async () => {
    draw();
    const user = userEvent.setup();
    await user.type(screen.getByRole('combobox', { name: 'Add members' }), 'zoe');
    const option = await screen.findByRole('option', { name: /Zoe Outsider/ });
    expect(option).toHaveTextContent('Person · Outside');
    expect(option).not.toHaveAttribute('aria-disabled', 'true');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('keeps the outsider warning and the refusal reason when sharing outside is unavailable', async () => {
    draw('Developers can add someone from outside.');
    const user = userEvent.setup();
    await user.type(screen.getByRole('combobox', { name: 'Add members' }), 'zoe');
    const option = await screen.findByRole('option', { name: /Zoe Outsider/ });
    expect(option).toHaveTextContent('Outside');
    expect(option).toHaveTextContent('Developers can add someone from outside.');
    expect(option).toHaveAttribute('aria-disabled', 'true');
    await user.keyboard('{Enter}');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('Enter takes the unique placed result and shows its human name', async () => {
    draw();
    const user = userEvent.setup();
    const field = screen.getByRole('combobox', { name: 'Add members' });
    await user.type(field, 'dep2 developer');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    expect(await screen.findByRole('option', { name: /Developers of Acme > dep2/ })).toBeVisible();
    await user.keyboard('{Enter}');
    expect(screen.getByRole('status')).toHaveTextContent('Chosen: Developers of Acme > dep2');
    expect(field).toHaveValue('Developers of Acme > dep2');
  });

  it('a loading search cannot be chosen, and an unknown locator says so', async () => {
    server.slow = true;
    draw();
    const user = userEvent.setup();
    const field = screen.getByRole('combobox', { name: 'Add members' });
    await user.type(field, 'dep2 developer');
    await waitFor(() => expect(server.release).not.toBeNull());
    await user.keyboard('{Enter}');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
    server.release?.(resolved);
    expect(await screen.findByRole('option', { name: /Developers of Acme > dep2/ })).toBeVisible();
    await user.clear(field);
    await user.type(field, 'dep2.nobody developer');
    expect(await screen.findByText('Nothing is called that here')).toBeVisible();
    await user.keyboard('{Enter}');
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });
});

it.each(['en', 'zh-CN'] as const)('offers recent placed people, first-key native filtering and continuation without losing the chosen operand in %s', async (locale) => {
  await setLocale(locale); client.session.setState({ userId: 'holder-reader', status: 'signed_in', readerVersion: 791 });
  server.pages = true; server.recent = [zoe.id]; const user = userEvent.setup(); draw();
  const input = screen.getByRole('combobox', { name: 'Add members' }); await user.click(input);
  const option = await screen.findByRole('option', { name: /Zoe Outsider/ });
  expect(screen.getAllByRole('option')[0]).toBe(option);
  expect(screen.getByRole('group', { name: i18n.t('ui.picker.recent') })).toBeVisible();
  await user.keyboard('{Home}{Enter}'); expect(screen.getByRole('status')).toHaveTextContent('Zoe Outsider');
  await waitFor(() => expect(server.record).toHaveBeenCalledWith({ kind: 'user', key: zoe.id }));
  await user.click(input); await user.clear(input); await user.type(input, 'z');
  await waitFor(() => expect(server.read).toHaveBeenCalledWith('z', null));
  await user.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') }));
  await waitFor(() => expect(server.read).toHaveBeenCalledWith('z', 'more'));
  expect(screen.getByRole('status')).toHaveTextContent('Zoe Outsider');
});
