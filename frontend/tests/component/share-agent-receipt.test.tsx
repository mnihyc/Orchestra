/** The real Share form acknowledges a committed receipt through the ordinary API helper, without resend. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';

import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { ShareWithAgentsDialog } from '@/features/access/ShareWithAgents';
import type { ShareAgentCreateReply, ShareAgentCreateReceipt, SharePreview } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { snapshot } from '../unit/continuation-fixtures';
import { ADA, CATALOGUE, pageFor } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ post: vi.fn(), get: vi.fn() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  // shareWithAgents, previewShare and client.ok stay real; only their raw transport answers are fixtures.
  return { ...actual, api: { ...actual.api, POST: calls.post, GET: calls.get } };
});

const session = client.session.getState();
afterEach(async () => {
  // Success notices use the shared store; dismiss through their real controls before unmounting.
  for (const close of screen.queryAllByRole('button', { name: i18n.t('common.dismiss') })) fireEvent.click(close);
  cleanup();
  client.session.setState(session);
  calls.post.mockReset();
  calls.get.mockReset();
  await setLocale('en');
});

const target = { kind: 'topic' as const, id: '01950000-0000-7000-8000-000000000070', name: 'Share source' };
const recipient = { ...snapshot().thread, name: 'Receiving thread' };
const acceptedId = '01950000-0000-7000-8000-000000000071';
const preview: SharePreview = {
  allowed: true, revision: 7, permissions: ['doc.view'], holders: [],
  effects: { pages: 1, orgs: null, gaining: [], losing: [], more: 0 },
  already: [], already_more: 0, agents: [], rows: [], rules: [], refusal: null,
};
const receipt: ShareAgentCreateReceipt = {
  projection: 'receipt',
  shares: [{ id: acceptedId, to: { kind: 'thread', id: recipient.id }, mode: 'live', created: true, input_id: null }],
};
const full: ShareAgentCreateReply = {
  shares: [{
    id: acceptedId, what: { id: target.id, kind: 'topic', name: target.name, redacted: false },
    to: { id: recipient.id, kind: 'thread', name: recipient.name, redacted: false },
    mode: 'live', inside: true, download: true, parts: { timeline: 'read', chat: 'read', documents: 'read' },
    read_first: false, viewer_entries: [], current: null, entries: [], snapshot: null, input: null,
    by: ADA, at: '2026-11-03T08:00:00Z',
  }],
};

function answer(data: unknown, status = 200) {
  return { data, response: new Response(null, { status, headers: { 'x-request-id': 'share-receipt-control' } }) };
}
const shareCalls = () => calls.post.mock.calls.filter(([path]) => path === '/share/agents');

it.each([
  ['write-only receipt', receipt],
  ['ordinary full reply', full],
] as const)('acknowledges the %s once, closes once, and never resends on Enter', async (_kind, reply) => {
  await setLocale('en');
  client.session.setState({ status: 'signed_in', userId: ADA.id });
  calls.post.mockImplementation((path: string) => {
    if (path === '/{object}/{id}/share/preview') return Promise.resolve(answer(preview));
    if (path === '/share/agents') return Promise.resolve(answer(reply, 201));
    throw new Error(`Unexpected POST in share receipt control: ${path}`);
  });
  calls.get.mockImplementation((path: string) => {
    if (path === '/permissions') return Promise.resolve(answer(CATALOGUE.reply));
    if (path === '/nodes/{id}') return Promise.resolve(answer(recipient));
    if (path === '/search/names') return Promise.resolve(answer({ items: [], partial: false }));
    throw new Error(`Unexpected GET in share receipt control: ${path}`);
  });
  const closed = vi.fn();
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  function Host() {
    const [open, setOpen] = useState(true);
    return <ShareWithAgentsDialog target={target} page={pageFor('topic')} open={open}
      initialRecipient={{ id: recipient.id, name: recipient.name }}
      onClose={() => { closed(); setOpen(false); }} />;
  }
  const root = createRootRoute({ component: () => <QueryClientProvider client={cache}><Host /></QueryClientProvider> });
  const router = createRouter({ routeTree: root, history: createMemoryHistory({ initialEntries: ['/'] }) });
  const shown = show(<RouterProvider router={router} />);
  try {
    const user = userEvent.setup();
    const submitName = i18n.t('access.agents.dialog.button.read', {
      who: i18n.t('access.agents.dialog.button.ofAgents', { name: recipient.name }),
      what: i18n.t('access.agents.dialog.button.this', { name: target.name }),
    });
    const submit = await screen.findByRole('button', { name: submitName });
    await waitFor(() => { expect(submit).toBeEnabled(); expect(submit).not.toHaveAttribute('aria-disabled', 'true'); });
    expect(shareCalls()).toHaveLength(0);
    const expectedShare: unknown = expect.objectContaining({ what: { node_id: target.id }, to: [{ thread_id: recipient.id }] });
    const expectedBody: unknown = expect.objectContaining({ kind: 'agents', share: expectedShare });
    expect(calls.post).toHaveBeenCalledWith('/{object}/{id}/share/preview', expect.objectContaining({
      params: { path: { object: 'nodes', id: target.id } },
      body: expectedBody,
    }));
    await user.click(submit);
    const success = i18n.t('access.agents.dialog.shared', { count: 1 });
    expect(await screen.findByText(success)).toBeVisible();
    expect(screen.getAllByText(success)).toHaveLength(1);
    await waitFor(() => expect(screen.queryByRole('dialog', { name: i18n.t('access.agents.dialog.title', { name: target.name }) })).not.toBeInTheDocument());
    expect(closed).toHaveBeenCalledOnce();
    expect(shareCalls()).toHaveLength(1);
    const anyClientId: unknown = expect.any(String);
    expect(shareCalls()[0]).toEqual(['/share/agents', { headers: { 'Idempotency-Key': anyClientId }, body: {
      what: { node_id: target.id }, mode: 'live', to: [{ thread_id: recipient.id }],
      parts: { timeline: 'read', chat: 'read', documents: 'read' }, inside: true,
      read_first: false, client_id: anyClientId,
    } }]);
    await user.keyboard('{Enter}');
    expect(closed).toHaveBeenCalledOnce();
    expect(shareCalls()).toHaveLength(1);
    expect(screen.queryByRole('button', { name: submitName })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('common.dismiss') }));
  } finally {
    for (const close of screen.queryAllByRole('button', { name: i18n.t('common.dismiss') })) fireEvent.click(close);
    shown.unmount();
    cache.clear();
  }
});
