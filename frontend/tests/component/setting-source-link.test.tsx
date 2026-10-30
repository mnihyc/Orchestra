/** Provenance links reuse known addresses and resolve missing ones only on intent, never by guessing names. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { show } from './support';

const server = vi.hoisted(() => ({ read: vi.fn<(id: string) => Promise<{ id: string; name: string; kind: 'org'; address: string }>>(), open: vi.fn() }));
vi.mock('@/api/nodes', async (load) => ({ ...await load<object>(), nodeQuery: (id: string) => ({
  queryKey: ['node', id], queryFn: () => server.read(id),
}) }));
vi.mock('@/features/chat/open', () => ({ useOpenPath: () => server.open }));
const def = { key: 'agent.instructions', class: 'parameter' } as const;
const target = { kind: 'node', id: 'web' } as const;
const source = { scope: 'org', node: { id: 'org-id', kind: 'org', name: 'Research', redacted: false } } as const;
beforeEach(() => {
  vi.clearAllMocks();
  server.read.mockResolvedValue({ id: 'org-id', kind: 'org', name: 'Research', address: 'acme.research' });
});
function draw(cached = false, byAddress = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  if (cached) client.setQueryData(byAddress ? ['org', 'acme.research'] : ['node', 'org-id'], { id: 'org-id', kind: 'org', name: 'Research', address: 'acme.research' });
  return show(<QueryClientProvider client={client}><SourceSettingLink source={source} def={def} target={target} fallback="Web" /></QueryClientProvider>);
}

it('uses an already served exact address as an ordinary link without another read', () => {
  draw(true);
  expect(screen.getByRole('link', { name: 'Set in Research' })).toHaveAttribute('href', '/org/acme.research/settings/policies?setting=agent.instructions');
  expect(server.read).not.toHaveBeenCalled();
  expect(server.open).not.toHaveBeenCalled();
});

it('reuses the native org-by-address read of a settings page without fetching that org by id', () => {
  draw(true, true);
  expect(screen.getByRole('link', { name: 'Set in Research' })).toHaveAttribute('href', '/org/acme.research/settings/policies?setting=agent.instructions');
  expect(server.read).not.toHaveBeenCalled();
});

it('does not fetch at mount, then reads the missing address once and opens only that answer', async () => {
  const user = userEvent.setup();
  draw();
  const link = screen.getByRole('link', { name: 'Set in Research' });
  expect(link).not.toHaveAttribute('href');
  expect(server.read).not.toHaveBeenCalled();
  link.focus();
  await user.keyboard('{Enter}');
  await waitFor(() => expect(server.read).toHaveBeenCalledExactlyOnceWith('org-id'));
  await waitFor(() => expect(server.open).toHaveBeenCalledExactlyOnceWith('/org/acme.research/settings/policies?setting=agent.instructions'));
});

it('does not use even a cached address for a concealed source', () => {
  const client = new QueryClient();
  client.setQueryData(['node', 'org-id'], { id: 'org-id', kind: 'org', address: 'secret.research' });
  const view = show(<QueryClientProvider client={client}><SourceSettingLink source={{ ...source, node: { ...source.node, name: null, redacted: true } }} def={def} target={target} fallback="Web" /></QueryClientProvider>);
  expect(screen.getByText('Set above')).toBeVisible();
  expect(view.container.querySelector('a[href*="secret"]')).toBeNull();
  expect(server.read).not.toHaveBeenCalled();
});

it('does not open a source after the row that requested it has gone away', async () => {
  let answer: (value: { id: string; name: string; kind: 'org'; address: string }) => void = () => { throw new Error('No source read is pending'); };
  const pending = new Promise<{ id: string; name: string; kind: 'org'; address: string }>((resolve) => { answer = resolve; });
  server.read.mockReturnValueOnce(pending);
  const view = draw();
  await userEvent.click(screen.getByRole('link', { name: 'Set in Research' }));
  await waitFor(() => expect(server.read).toHaveBeenCalledOnce());
  view.unmount();
  await act(async () => { answer({ id: 'org-id', kind: 'org', name: 'Research', address: 'acme.research' }); await pending; });
  expect(server.open).not.toHaveBeenCalled();
});

it('does not redirect a later navigation when an earlier source lookup finishes', async () => {
  const before = location.href;
  let answer: (value: { id: string; name: string; kind: 'org'; address: string }) => void = () => { throw new Error('No source read is pending'); };
  const pending = new Promise<{ id: string; name: string; kind: 'org'; address: string }>((resolve) => { answer = resolve; });
  server.read.mockReturnValueOnce(pending);
  const view = draw();
  try {
    await userEvent.click(screen.getByRole('link', { name: 'Set in Research' }));
    await waitFor(() => expect(server.read).toHaveBeenCalledOnce());
    history.replaceState(null, '', '/topic/another-place');
    await act(async () => { answer({ id: 'org-id', kind: 'org', name: 'Research', address: 'acme.research' }); await pending; });
    expect(server.open).not.toHaveBeenCalled();
  } finally { view.unmount(); history.replaceState(null, '', before); }
});
