/** The explicit send choice owns grants, retries and accepted receipts; typing owns none. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import type * as LinkPlans from '@/features/sharing/linkPlan';
import type * as PersonActions from '@/api/personActions';
import type * as Share from '@/api/share';
import { ReceiptAcknowledged } from '@/api/client';
import { LinkSharing, useLinkSharing } from '@/features/sharing/LinkSharing';
import type { LinkContext, LinkPlan } from '@/features/sharing/linkPlan';
import type { NodeRef, ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { Button } from '@/ui/Button';
import { BEA } from '../unit/general-access-fixtures';
import { show } from './support';

const DOC = '00000000-0000-4000-8000-000000000091';
const CHAT = '00000000-0000-4000-8000-000000000092';
const agent: NodeRef = { id: '00000000-0000-4000-8000-000000000093', kind: 'thread', name: 'Reviewer', redacted: false };
const context: LinkContext = { origin: { kind: 'chat', chat_id: CHAT }, placeId: CHAT, sender: 'actor' };
const parts = [{ type: 'text' as const, text: `[Plan](/doc/${DOC})` }];
const policy: ResolvedSetting = { key: 'sharing.on_link', value: true, own: null, locked_by: null, source: { scope: 'default', node: null }, parts: [], writable: false };
const calls = vi.hoisted(() => ({ get: vi.fn<(path: string, ...options: unknown[]) => Promise<{ response: Response; data: unknown }>>(), preview: vi.fn<typeof LinkPlans.previewLinkPlan>(), give: vi.fn<typeof PersonActions.grantPerson>(), agent: vi.fn<typeof Share.shareWithAgents>(), submit: vi.fn<() => void>(),
  preference: true, locked: false, changed: false, personGiven: false, setReader: (_version: number): void => undefined,
}));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof Client>();
  const { createStore } = await import('zustand/vanilla');
  const session = createStore(() => ({ userId: 'actor', readerVersion: 1 }));
  calls.setReader = (readerVersion) => session.setState({ readerVersion });
  return { ...actual, client: { ...actual.client, session }, api: { ...actual.api, GET: (path: string, ...options: unknown[]) => calls.get(path, ...options) } };
});
vi.mock('@/features/sharing/linkPlan', async (original) => {
  const actual = await original<typeof LinkPlans>();
  return { ...actual, previewLinkPlan: (...args: Parameters<typeof LinkPlans.previewLinkPlan>) => calls.preview(...args) };
});
vi.mock('@/api/personActions', async (original) => ({ ...await original<typeof PersonActions>(), grantPerson: (...args: Parameters<typeof PersonActions.grantPerson>) => calls.give(...args) }));
vi.mock('@/api/share', async (original) => ({ ...await original<typeof Share>(), shareWithAgents: (...args: Parameters<typeof Share.shareWithAgents>) => calls.agent(...args) }));
vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user: { display_name: string } }) => <span>{user.display_name}</span> }));
vi.mock('@/features/people/NamedChip', () => ({ KnownNodeChip: ({ node }: { node: NodeRef }) => <span>{node.name}</span>, NamedChip: ({ original }: { original: string }) => <span>{original}</span> }));

function plan(keyFor: (key: string) => string): LinkPlan {
  const target = { kind: 'document' as const, id: DOC, name: 'Plan' };
  const gifts: LinkPlan['gifts'] = calls.personGiven ? [] : [{ kind: 'person', key: 'person-gift', target, person: BEA, choice: 'full_view', participation: false }];
  if (!calls.changed) gifts.push({ kind: 'agent', key: 'agent-gift', target, agent, share: { client_id: keyFor('agent-gift'), what: { document_id: DOC }, to: [{ thread_id: agent.id }], mode: 'live', parts: { documents: 'read' }, download: false, viewers_read: false, read_first: false, link_origin: context.origin } });
  return { gifts, people: calls.personGiven ? [] : [BEA], agents: calls.changed ? [] : [agent], memberships: [], unknown: false, placeId: CHAT };
}
function Harness() {
  const sharing = useLinkSharing(context, parts);
  return <><LinkSharing sharing={sharing} /><Button disabled={sharing.blocked} onClick={() => void sharing.send(calls.submit)}>Send</Button></>;
}
const caption = () => i18n.t('linkSharing.label', { people: i18n.t('linkSharing.people', { count: 1, number: '1' }), agents: i18n.t('linkSharing.agents', { count: 1, number: '1' }) });
beforeEach(() => {
  vi.clearAllMocks(); calls.setReader(1); calls.preference = true; calls.locked = false; calls.changed = false; calls.personGiven = false;
  calls.get.mockImplementation((path: string) => Promise.resolve({ response: new Response(null, { status: 200 }), data: path === '/me/state'
    ? { timeline: { share_on_link: calls.preference } }
    : [{ ...policy, value: !calls.locked, locked_by: calls.locked ? { scope: 'system', node: null } : null }] }));
  calls.preview.mockImplementation((_ctx, _refs, _parts, keyFor) => Promise.resolve(plan(keyFor)));
  calls.give.mockResolvedValue(undefined); calls.agent.mockResolvedValue({ shares: [] });
});
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('grants only after the selected send act and carries native origin in %s', async (locale) => {
  await setLocale(locale); const user = userEvent.setup(); show(<Harness />);
  expect(await screen.findByRole('switch', { name: caption() })).toBeChecked();
  expect(calls.give).not.toHaveBeenCalled(); expect(calls.agent).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(calls.submit).toHaveBeenCalledOnce());
  expect(calls.give).toHaveBeenCalledWith({ kind: 'document', id: DOC, name: 'Plan' }, BEA.id, 'full_view', false, expect.any(String), context.origin);
  expect(calls.agent.mock.calls[0]?.[0]).toMatchObject({ link_origin: context.origin, viewers_read: false, read_first: false, download: false });
  expect(calls.agent.mock.calls[0]?.[0].link_origin).not.toHaveProperty('by');
  expect(screen.getByText(i18n.t('linkSharing.shared'))).toBeInTheDocument();
});

it('lets the sender turn the choice off without granting', async () => {
  const user = userEvent.setup(); show(<Harness />);
  await user.click(await screen.findByRole('switch', { name: caption() }));
  await user.click(screen.getByRole('button', { name: 'Send' }));
  expect(calls.submit).toHaveBeenCalledOnce(); expect(calls.give).not.toHaveBeenCalled(); expect(calls.agent).not.toHaveBeenCalled();
});

it('starts unticked for the personal opt-out and respects a locked-off place', async () => {
  calls.preference = false; const first = show(<Harness />);
  expect(await screen.findByRole('switch', { name: caption() })).not.toBeChecked(); first.unmount();
  calls.preference = true; calls.locked = true; show(<Harness />);
  expect(await screen.findByText(i18n.t('linkSharing.lockedOff'))).toBeInTheDocument();
  expect(screen.queryByRole('switch')).toBeNull();
  await userEvent.click(screen.getByRole('button', { name: 'Send' }));
  expect(calls.give).not.toHaveBeenCalled(); expect(calls.agent).not.toHaveBeenCalled();
});

it('asks for review rather than writing a fresh changed recipient set', async () => {
  show(<Harness />); await screen.findByRole('switch', { name: caption() }); calls.changed = true;
  await userEvent.click(screen.getByRole('button', { name: 'Send' }));
  expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('linkSharing.changed'));
  expect(calls.submit).not.toHaveBeenCalled(); expect(calls.give).not.toHaveBeenCalled(); expect(calls.agent).not.toHaveBeenCalled();
});

it('accepts a committed restricted grant receipt without resending for readback', async () => {
  calls.give.mockRejectedValueOnce(new ReceiptAcknowledged({ projection: 'receipt', defaults: false, kind: 'document', object_id: DOC, revision: 2 }, 200, 'receipt'));
  show(<Harness />); await screen.findByRole('switch', { name: caption() });
  await userEvent.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(calls.submit).toHaveBeenCalledOnce()); expect(calls.give).toHaveBeenCalledOnce();
});

it('retains partial success and the unknown outcome key while leaving the draft unsent', async () => {
  calls.give.mockImplementation(() => { calls.personGiven = true; return Promise.resolve(); });
  calls.agent.mockRejectedValueOnce(new Error('Lost response')).mockResolvedValueOnce({ shares: [] });
  show(<Harness />); await screen.findByRole('switch', { name: caption() });
  await userEvent.click(screen.getByRole('button', { name: 'Send' })); await screen.findByRole('alert');
  expect(calls.submit).not.toHaveBeenCalled(); expect(screen.getByText(i18n.t('linkSharing.shared'))).toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('linkSharing.changed')));
  await userEvent.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(calls.submit).toHaveBeenCalledOnce());
  expect(calls.give).toHaveBeenCalledOnce(); expect(calls.agent).toHaveBeenCalledTimes(2);
  expect(calls.agent.mock.calls[0]?.[0].client_id).toBe(calls.agent.mock.calls[1]?.[0].client_id);
});

it('stops remaining gifts and send when the reader changes during a held write', async () => {
  let finish: (() => void) | undefined;
  calls.give.mockImplementation(() => new Promise<void>((resolve) => { finish = resolve; }));
  show(<Harness />); await screen.findByRole('switch', { name: caption() });
  await userEvent.click(screen.getByRole('button', { name: 'Send' }));
  await waitFor(() => expect(calls.give).toHaveBeenCalledOnce());
  const settle = finish; if (settle === undefined) throw new Error('The gift did not start');
  await act(async () => { calls.setReader(2); settle(); await Promise.resolve(); });
  expect(calls.agent).not.toHaveBeenCalled(); expect(calls.submit).not.toHaveBeenCalled();
});
