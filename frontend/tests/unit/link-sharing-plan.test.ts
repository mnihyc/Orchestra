/** Native before-state and a complete readable audience determine the proposed gifts. */
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import type * as Access from '@/api/access';
import type * as People from '@/api/personActions';
import { carriedLinks, previewLinkPlan } from '@/features/sharing/linkPlan';
import type { ChatMentionPage, NodeRef, SharePreview } from '@/generated/api-types';
import { referenceTo } from '@/lib/links';
import { ADA, BEA } from './general-access-fixtures';
import { chat } from '../doubles/chat-rows';
import type { HttpTransport } from '../doubles/http-wire';

const DOC = '00000000-0000-4000-8000-000000000081';
const AGENT: NodeRef = { id: '00000000-0000-4000-8000-000000000082', kind: 'thread', name: 'Reviewer', redacted: false };
const CHAT = '00000000-0000-4000-8000-000000000083';
const calls = vi.hoisted(() => ({ get: vi.fn<HttpTransport>(), person: vi.fn<typeof People.readPersonActions>(), preview: vi.fn<typeof Access.previewShare>() }));
vi.mock('@/api/client', async (original) => { const actual = await original<typeof Client>(); return { ...actual, api: { ...actual.api, GET: calls.get } }; });
vi.mock('@/api/personActions', async (original) => ({ ...await original<typeof People>(), readPersonActions: (...args: Parameters<typeof People.readPersonActions>) => calls.person(...args) }));
vi.mock('@/api/access', async (original) => ({ ...await original<typeof Access>(), previewShare: (...args: Parameters<typeof Access.previewShare>) => calls.preview(...args) }));
const answer = (data: unknown) => ({ data, response: new Response(null, { status: 200 }) });
const choice = (user: typeof ADA) => ({ target: { kind: 'user' as const, id: user.id }, user, name: user.display_name, available: true, reason: null, count: 1, thread: null, agent: null, state: null, started_at: null });
const parts = [{ type: 'text' as const, text: `[Plan](/doc/${DOC})` }];
const refs = carriedLinks(parts);
const signal = () => new AbortController().signal;
function trial(readable: boolean | null): SharePreview {
  const agent: SharePreview['agents'][number] = { to: AGENT, inside: false, parts: { documents: 'read' as const }, already_shared: {}, download: false, future_threads: false, copy: null,
    effective_before: { readable, reason: readable === null ? 'recipient_context_required' : null } };
  return { allowed: true, agents: [agent], already: [], already_more: 0, effects: { gaining: [], losing: [], more: 0, orgs: 1, pages: 1 }, holders: [], permissions: [], refusal: null, revision: 1, rows: [], rules: [] };
}
beforeEach(() => { vi.clearAllMocks(); calls.person.mockResolvedValue({ can_view: false, can_notify: false, gives: ['full_view'], note_chars: 0 }); });

it('reads all audience pages and excludes the sender and an already readable recipient', async () => {
  const other = { ...BEA, id: '00000000-0000-4000-8000-000000000084', display_name: 'Chen' };
  calls.get.mockImplementation((path, options) => {
    if (path === '/chats/{id}') return Promise.resolve(answer(chat({ id: CHAT, agents: [] })));
    const page: ChatMentionPage = options.params?.query?.cursor === undefined
      ? { items: [choice(ADA), choice(BEA)], followers: 3, next_cursor: 'next' }
      : { items: [choice(other)], followers: 3, next_cursor: null };
    return Promise.resolve(answer(page));
  });
  calls.person.mockImplementation((_target, user) => Promise.resolve({ can_view: user === BEA.id, can_notify: false, gives: ['full_view'], note_chars: 0 }));
  const plan = await previewLinkPlan({ origin: { kind: 'chat', chat_id: CHAT }, placeId: 'org', sender: ADA.id }, refs, parts, () => 'key', signal());
  expect(plan.people.map((person) => person.id)).toEqual([other.id]);
  expect(calls.person).toHaveBeenCalledTimes(2);
  expect(calls.get.mock.calls.some(([, options]) => options.params?.query?.cursor === 'next')).toBe(true);
});

it.each([true, false, null])('uses the native agent baseline %s rather than this giver’s old ceiling', async (readable) => {
  calls.preview.mockResolvedValue(trial(readable));
  const plan = await previewLinkPlan({ origin: { kind: 'thread_input', thread_id: AGENT.id }, placeId: AGENT.id, sender: ADA.id, agent: AGENT }, refs, parts, () => 'gift-key', signal());
  expect(plan.agents.length).toBe(readable === false ? 1 : 0);
  expect(plan.unknown).toBe(readable === null);
  if (readable === false) expect(plan.gifts[0]).toMatchObject({ kind: 'agent', share: { mode: 'live', download: false, read_first: false, viewers_read: false, link_origin: { kind: 'thread_input', thread_id: AGENT.id } } });
});

it('keeps the native group membership choice instead of inventing a read-only member', async () => {
  calls.get.mockImplementation((path, options) => {
    if (path !== '/chats/{id}') return Promise.resolve(answer({ items: [choice(BEA)], followers: 1, next_cursor: null }));
    const id = options.params?.path?.id;
    if (id === undefined) throw new Error('The actual chat request must name its path id');
    return Promise.resolve(answer(chat({ id, kind: 'group', name: 'Planning', agents: [] })));
  });
  calls.person.mockResolvedValue({ can_view: false, can_notify: false, gives: ['full_interact'], note_chars: 0 });
  const quote = [{ id: DOC, kind: 'chat' as const, label: 'Planning' }];
  const plan = await previewLinkPlan({ origin: { kind: 'chat', chat_id: CHAT }, placeId: 'org', sender: ADA.id }, quote, parts, () => 'key', signal());
  expect(plan.gifts[0]).toMatchObject({ kind: 'person', choice: 'full_interact', participation: true });
  expect(plan.memberships.map((target) => target.id)).toEqual([DOC]);
});

it('recognizes the copied grammar with escaped labels and spaces while excluding code and foreign links', () => {
  const reference = referenceTo({ kind: 'environment_file', id: DOC, path: 'src/a b.ts', name: 'Plan [v2]' }, { line: 7 });
  expect(carriedLinks([{ type: 'text', text: reference }])).toMatchObject([{ id: DOC, path: 'src/a b.ts' }]);
  expect(carriedLinks([{ type: 'text', text: `\`${reference}\`\n\n\`\`\`\n[Plan](id:${DOC})\n\`\`\`\nhttps://elsewhere.invalid/doc/${DOC}` }])).toEqual([]);
});
