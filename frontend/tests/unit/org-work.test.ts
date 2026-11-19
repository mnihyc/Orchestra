import { display } from '../doubles/thread-display';
import { executionDisplay } from '../doubles/thread-display';
/** Daily work is personal, named from current authority, ordered by visits and explained in words. */
import { afterEach, expect, it } from 'vitest';

import { orgWork, workReason } from '@/features/org/workModel';
import type { NameHit, TreeNode } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import type { OrgTree } from '@/realtime/stores/tree';
import { ACME, acmeTree } from '../doubles/fixtures';

function treeWith(changes: readonly TreeNode[] = []): OrgTree {
  const nodes = new Map<string, TreeNode>(acmeTree().map((node) => [node.id, { ...node, participating: false }]));
  for (const node of changes) nodes.set(node.id, node);
  return { nodes, loaded: true, lastSeq: 100, error: null };
}

function thread(id: string, name: string, more: Partial<NonNullable<TreeNode['thread']>> = {}, row: Partial<TreeNode> = {}): TreeNode {
  const base = acmeTree().find((node) => node.id === ACME.mainThread);
  if (base?.thread === undefined || base.thread === null) throw new Error('the fixture has a thread');
  return { ...base, id, name, participating: false, thread: { ...base.thread, state: 'idle', display: executionDisplay(more.state ?? 'idle', { blocked_reason: more.blocked_reason ?? null, ...(more.attention?.failed ? { state: 'failed' } : {}) }), ...more }, ...row };
}

function visit(id: string, more: Partial<NameHit> = {}): NameHit {
  return { id, kind: 'thread', name: 'A stale name must never be used', address: null, avatar: null, owner: null,
    username: null, path: [], archived: false, recent: true, tag: null, color: null, custom_color: null, thread_attention: null, ...more };
}

afterEach(async () => { await setLocale('en'); });

it('keeps visit order, never activity order, and names each current readable thread only once', () => {
  const one = thread(ACME.mainThread, 'Hunt the login bug', { last_activity_at: '2026-10-15T11:00:00Z' });
  const two = thread(ACME.depsThread, 'Upgrade dependencies', { last_activity_at: '2026-10-15T18:00:00Z' });
  const waiting = thread(ACME.repro, 'Deploy staging', { state: 'waiting_approval', waiting: { kind: 'approval', for_caller: true } });
  const work = orgWork(treeWith([one, two, waiting]), ACME.org, [visit(one.id), visit(waiting.id), visit(two.id), visit(one.id)]);
  expect(work.recent.map(({ node }) => node.name)).toEqual(['Hunt the login bug', 'Upgrade dependencies']);
  expect(work.waiting.map(({ node }) => node.name)).toEqual(['Deploy staging']);
  expect(work.recent[0]?.where).toBe('Research / Web / Fix login');
  expect(work.recent[0]?.where).not.toContain('Acme');
});

it('leads with actual requests for this reader, then their participating or visited waits, never admin-only work', () => {
  const own = thread(ACME.repro, 'Your decision', { state: 'waiting_approval', waiting: { kind: 'approval', for_caller: true }, last_activity_at: '2026-10-15T10:00:00Z' });
  const participating = thread(ACME.idleThread, 'Your blocked work', { state: 'blocked', blocked_reason: 'environment_unavailable' }, { participating: true });
  const visited = thread(ACME.depsThread, 'A watched question', { state: 'waiting_approval', waiting: { kind: 'answer', for_caller: false } });
  const admin = thread(ACME.baselineThread, 'Somebody else’s work', { state: 'waiting_approval', waiting: { kind: 'approval', for_caller: false } }, { admin_access: { place: 'system', private: true } });
  const work = orgWork(treeWith([own, participating, visited, admin]), ACME.org, [visit(visited.id)]);
  expect(work.waiting.map(({ node }) => node.id)).toContain(own.id);
  expect(work.waiting[0]?.node.id).toBe(own.id);
  expect(work.waiting.map(({ node }) => node.id)).toContain(participating.id);
  expect(work.waiting.map(({ node }) => node.id)).toContain(visited.id);
  expect(work.waiting.map(({ node }) => node.id)).not.toContain(admin.id);
});

it('drops revoked, deleted, archived, unrelated and non-recent hits even if the names lookup still holds them', () => {
  const deleted = thread(ACME.mainThread, 'Deleted', {}, { deleted: true });
  const archived = thread(ACME.repro, 'Archived', {}, { archived: true });
  const ordinary = thread(ACME.depsThread, 'Never visited');
  const work = orgWork(treeWith([deleted, archived, ordinary]), ACME.org, [
    visit(deleted.id), visit(archived.id), visit(ordinary.id, { recent: false }), visit('revoked'), visit('other-org'), visit(ACME.web, { kind: 'project' }),
  ]);
  expect(work).toEqual({ waiting: [], recent: [] });
});

it('keeps a department’s work within that department, while a local fallback still needs current tree access', () => {
  const inside = thread(ACME.mainThread, 'Department work');
  const outside = thread(ACME.idleThread, 'Root work', {}, { parent_id: ACME.idleStop });
  const work = orgWork(treeWith([inside, outside]), ACME.research, [], [outside.id, 'revoked', inside.id]);
  expect(work.recent.map(({ node }) => node.name)).toEqual(['Department work']);
  expect(work.recent[0]?.where).toBe('Web / Fix login');
});

it('puts a newly opened local thread ahead of an older cached visit answer, with no duplicate', () => {
  const newest = thread(ACME.mainThread, 'Just opened');
  const older = thread(ACME.depsThread, 'Opened earlier');
  const work = orgWork(treeWith([newest, older]), ACME.org, [visit(older.id), visit(newest.id)], [newest.id]);
  expect(work.recent.map(({ node }) => node.name)).toEqual(['Just opened', 'Opened earlier']);
});

it('keeps a directly shared recent thread when its private parent is absent, without borrowing a cached parent name', () => {
  const shared = thread('shared-thread', 'Review the shared result', {}, { parent_id: 'private-topic' });
  const tree = treeWith([shared]);
  const recent = visit(shared.id, { path: [
    { id: ACME.org, kind: 'org', name: 'Acme', redacted: false },
    { id: ACME.research, kind: 'org', name: 'Research', redacted: false },
    { id: 'private-topic', kind: 'topic', name: 'Old private name', redacted: false },
  ] });
  expect(orgWork(tree, ACME.org, [recent]).recent.map(({ node }) => node.name)).toEqual(['Review the shared result']);
  const department = orgWork(tree, ACME.research, [recent]);
  expect(department.recent).toHaveLength(1);
  expect(department.recent[0]?.where).toBe('');
  const revoked = { ...tree, nodes: new Map(tree.nodes) };
  revoked.nodes.delete(shared.id);
  expect(orgWork(revoked, ACME.research, [recent]).recent).toEqual([]);
});

it.each(['en', 'zh-CN'] as const)('uses current authority and distinct question/block causes in %s without diagnostic keys', async (locale) => {
  await setLocale(locale);
  const t = i18n.getFixedT(locale);
  const one = thread(ACME.repro, 'Deploy', { state: 'waiting_approval', waiting: { kind: 'approval', for_caller: true } });
  expect(workReason(one, t)).toBe(locale === 'en' ? 'Waiting for your approval' : '等待你批准');
  const viewer = thread(ACME.repro, 'Deploy', { state: 'waiting_approval', waiting: { kind: 'approval', for_caller: false } });
  expect(workReason(viewer, t)).toBe(locale === 'en' ? 'Waiting for approval' : '等待批准');
  const question = thread(ACME.repro, 'Question', { waiting: { kind: 'answer', for_caller: true } });
  expect(workReason(question, t)).toBe(locale === 'en' ? 'Waiting for your answer' : '等待你回答');
  expect(workReason(thread(ACME.repro, 'Question', { waiting: { kind: 'answer', for_caller: false } }), t)).toBe(t('places.org.work.answer'));
  expect(workReason(thread(ACME.idleThread, 'Sandbox', { state: 'blocked', blocked_reason: 'binding_invalid', blocked_cause: 'sandbox' }), t)).toBe(locale === 'en' ? 'Blocked: no suitable sandbox' : '已阻塞：没有适用的沙箱');
  const reasons = ['unbound', 'agent_offline', 'agent_disabled', 'provider_unavailable', 'provider_exhausted', 'environment_unavailable', 'binding_invalid', 'provider_not_allowed', 'model_not_set', 'model_not_allowed', 'quota_exceeded', 'attachments', 'workspace_trust', 'switching', 'error'] as const;
  for (const blocked_reason of reasons) {
    const text = workReason(thread(ACME.idleThread, 'Blocked', { state: 'blocked', blocked_reason }), t);
    expect(text).toMatch(locale === 'en' ? /^Blocked: / : /^已阻塞：/);
    expect(text).not.toMatch(/places\.|sandbox\.mode|agent\.max_sandbox|binding_invalid|provider_exhausted/);
  }
});

it('a live decision, rename and removal change the existing list rather than repeating the old waiting row', () => {
  const original = thread(ACME.repro, 'Deploy staging', { state: 'waiting_approval', waiting: { kind: 'approval', for_caller: true } });
  const before = treeWith([original]);
  expect(orgWork(before, ACME.org, [visit(original.id)]).waiting).toHaveLength(1);
  const current = thread(original.id, 'Deploy production', { state: 'idle', waiting: null });
  const after = treeWith([current]);
  const work = orgWork(after, ACME.org, [visit(original.id)]);
  expect(work.waiting).toEqual([]);
  expect(work.recent.map(({ node }) => node.name)).toEqual(['Deploy production']);
  expect(orgWork(treeWith([{ ...current, deleted: true }]), ACME.org, [visit(current.id)])).toEqual({ waiting: [], recent: [] });
});

it('puts held inputs and failed work in the waiting list from their display facts', () => {
  const held = thread(ACME.mainThread, 'Held messages', { state: 'idle', display: display('held') }, { participating: true });
  const failed = thread(ACME.depsThread, 'Failed work', { state: 'idle', display: display('failed') }, { participating: true });
  const queued = thread(ACME.repro, 'Scheduled work', { state: 'idle', display: display('queued') }, { participating: true });
  const work = orgWork(treeWith([held, failed, queued]), ACME.org, []);
  expect(work.waiting.map(({ node }) => node.name).sort()).toEqual(['Failed work', 'Held messages']);
});
