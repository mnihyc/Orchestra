/** A locator resolves to an accurate, placed holder; typing or loading alone never gives it. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { type Candidate, useCandidates } from '@/features/access/candidates';
import type { HolderSearchReply } from '@/generated/api-types';
import { Combobox } from '@/ui/Combobox';

import { show } from './support';

const server = vi.hoisted(() => ({ slow: false, release: null as ((reply: HolderSearchReply) => void) | null }));
const dep2 = '00000000-0000-4000-8000-000000000002';
const resolved: HolderSearchReply = { items: [{ input: { kind: 'role', role: 'developer', at: dep2 },
  holder: { kind: 'role', role: 'developer', at: { id: dep2, kind: 'org', name: 'Acme > dep2', redacted: false }, place: 'Acme > dep2', count: 1 },
  locator: 'acme.dep2 developer', place: 'Acme > dep2', outside: false }] };

vi.mock('@/api/access', async (original) => ({ ...await original<object>(),
  holderSearchQuery: (q: string) => ({ queryKey: ['holder-picker-proof', q], queryFn: (): Promise<HolderSearchReply> => {
    if (q !== 'dep2 developer') return Promise.resolve({ items: [] });
    if (server.slow) return new Promise((resolve) => { server.release = resolve; });
    return Promise.resolve(resolved);
  } }),
}));

function Picker() {
  const [query, setQuery] = useState('');
  const [chosen, setChosen] = useState<Candidate | null>(null);
  const found = useCandidates(query, null, { kinds: ['user', 'group', 'role', 'members'], thing: 'dep3', placeId: dep2 });
  return <>
    <Combobox label="Add members" query={query} onQueryChange={setQuery} groups={found.groups} loading={found.loading}
      empty="Nothing is called that here" onSelect={(option) => {
        const holder = found.byValue.get(option.value);
        if (holder !== undefined) { setChosen(holder); setQuery(holder.name); }
      }} />
    {chosen !== null && <p role="status">Chosen: {chosen.name}</p>}
  </>;
}

function draw() {
  return show(<QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}><Picker /></QueryClientProvider>);
}

beforeEach(() => { server.slow = false; server.release = null; });

describe('the holder locator', () => {
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
