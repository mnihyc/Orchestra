import { treeNode } from '../doubles/tree-node';
// @vitest-environment node
/// <reference types="node" />
/** Every act of the application is in the registry of acts, with what it changes; a change reaches exactly the queries it
 * names; and what the live connection says reaches the views through the same changes. The first is a check over the
 * sources: every request that changes something, as the code writes it, is found and looked up as the client would
 * look it up when the act succeeds. */
import { readFileSync, readdirSync } from 'node:fs';
import { relative, resolve } from 'node:path';
import { QueryClient, QueryObserver, type QueryKey } from '@tanstack/react-query';
import ts from 'typescript';
import { describe, expect, it, vi } from 'vitest';

import { ACTS, applyChanges, type Change, covers, findAct, followWith, resourceChanges, threadRowChanges, treeChanges, userEventChanges } from '@/api/acts';
import { browsersQuery } from '@/api/browsers';
import { createApiClient } from '@/api/client';
import { displaysQuery } from '@/api/displays';
import { duplicateObjects } from '@/api/duplicates';
import { inputScheduleQuery, inputScheduleRunsQuery, topicSchedulesQuery } from '@/api/inputSchedules';
import { documentLinksQuery } from '@/api/links';
import { moveOutPath, supportsMoveOut } from '@/api/moveOut';
import { portsKey } from '@/api/ports';
import { documentSummaryOptionsKey, resourceSummaryKey } from '@/api/summaries';
import type { DocumentLinks, MachinePrograms, TreeNode } from '@/generated/api-types';
import type { HomeMoveKind } from '@/features/duplicate/move';
import { me, org, userRef } from '../doubles/fixtures';
import { inputSchedule } from '../doubles/input-schedules';

const sourceRoot = resolve(import.meta.dirname, '../../src');
/** Parsing all source once takes seconds on the shared builder; this bounds the scan. */
const SCAN_BUDGET_MS = 20_000;
/** What a parameter of a path stands for when a request written in the code is looked up: any one segment. */
const ANY = 'x';
const WRITES = new Set(['POST', 'PUT', 'PATCH', 'DELETE']);
const moveFamilies: Record<HomeMoveKind, unknown> = { ...duplicateObjects, host: null, machine: null };
// Run the real finite builder over every declared family and draft state. Unported
// families remain rejected by its own predicate; every returned path must have an act.
const movePaths = [...new Set((Object.keys(moveFamilies) as HomeMoveKind[])
  .filter(supportsMoveOut)
  .flatMap((kind) => [false, true].map((draft) => moveOutPath({ kind, id: ANY, draft }))))];

it.each(['PUT', 'DELETE'])('repairs visible native parameters omitted by a %s setting receipt without reading another thread', async (method) => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const before = { parameters: [{ key: 'web_search', value: null }] };
  const after = { parameters: [{ key: 'web_search', value: method === 'PUT' ? 'service' : null }] };
  const key = ['thread-parameters', 'edited-thread'], other = ['thread-parameters', 'other-thread'];
  const read = vi.fn(() => Promise.resolve(after)), otherRead = vi.fn(() => Promise.resolve(before));
  cache.setQueryData(key, before); cache.setQueryData(other, before); cache.setQueryData(['settings', 'edited-thread'], []);
  const stops = [
    new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity }).subscribe(() => undefined),
    new QueryObserver(cache, { queryKey: other, queryFn: otherRead, staleTime: Infinity }).subscribe(() => undefined),
  ];
  const answer = { key: 'agent.params', value: method === 'PUT' ? { web_search: 'service' } : {}, source: { scope: 'thread', node: null }, own: null, locked_by: null, writable: true, parts: [] };
  try {
    const act = findAct(method, '/nodes/edited-thread/settings/agent.params', { answer });
    expect(act).not.toBeNull();
    await applyChanges(act?.changes ?? [], cache);
    expect(cache.getQueryData(key)).toEqual(after);
    expect(cache.getQueryData(other)).toEqual(before);
    expect(cache.getQueryData(['settings', 'edited-thread'])).toEqual([answer]);
    expect(read).toHaveBeenCalledTimes(1); expect(otherRead).not.toHaveBeenCalled();
  } finally { stops.forEach((stop) => stop()); cache.clear(); }
});

it.each(['PUT', 'DELETE'])('expires hidden native parameters after a %s setting receipt without reading them', async (method) => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const key = ['thread-parameters', 'hidden-thread'];
  const read = vi.fn(() => Promise.resolve({ parameters: [] }));
  cache.setQueryDefaults(key, { queryFn: read });
  cache.setQueryData(key, { parameters: [{ key: 'web_search', value: null }] });
  try {
    const act = findAct(method, '/nodes/hidden-thread/settings/agent.params', { answer: { key: 'agent.params', value: {} } });
    expect(act).not.toBeNull();
    await applyChanges(act?.changes ?? [], cache);
    expect(cache.getQueryState(key)).toBeUndefined(); expect(read).not.toHaveBeenCalled();
  } finally { cache.clear(); }
});

it('keeps a personal webhook in its own channel list through creation, editing and removal without another read', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const key = ['notification-channels'];
  const read = vi.fn(() => Promise.resolve([]));
  const other = { id: 'org-channel', kind: 'webhook', name: 'Team room', owner: { org: { id: 'org' } } };
  cache.setQueryData(key, []);
  cache.setQueryData(['channels', 'org'], [other]);
  cache.setQueryData(['channels', 'system'], []);
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity }).subscribe(() => undefined);
  const channel = { id: 'my-channel', kind: 'webhook', name: 'My room', owner: 'me', route: { enabled: false } };
  try {
    await applyChanges(findAct('POST', '/me/notification-channels', { answer: channel })?.changes ?? [], cache);
    expect(cache.getQueryData(key)).toEqual([channel]);
    const edited = { ...channel, name: 'Our room' };
    await applyChanges(findAct('PATCH', '/notification-channels/my-channel', { answer: edited })?.changes ?? [], cache);
    expect(cache.getQueryData(key)).toEqual([edited]);
    await applyChanges(findAct('DELETE', '/notification-channels/my-channel')?.changes ?? [], cache);
    expect(cache.getQueryData(key)).toEqual([]);
    expect(cache.getQueryData(['channels', 'org'])).toEqual([other]);
    expect(cache.getQueryData(['channels', 'system'])).toEqual([]);
    expect(read).not.toHaveBeenCalled();
  } finally { stop(); cache.clear(); }
});

it('keeps an initial personal channel read running until its complete native baseline arrives', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const key = ['notification-channels'];
  const channel = { id: 'my-channel', kind: 'webhook', name: 'My room', owner: 'me', route: { enabled: false } };
  const existing = { id: 'mail', kind: 'email', name: 'Mail', owner: 'system' };
  let deliver!: (rows: unknown[]) => void;
  const pending = new Promise<unknown[]>((resolve) => { deliver = resolve; });
  const read = vi.fn(() => pending);
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
  const stop = observer.subscribe(() => undefined);
  const completed = cache.getQueryCache().find({ queryKey: key, exact: true })?.promise?.then(() => undefined, () => undefined);
  try {
    expect(completed).toBeDefined();
    await applyChanges(findAct('POST', '/me/notification-channels', { answer: channel })?.changes ?? [], cache);
    expect(observer.getCurrentResult().isFetching).toBe(true);
    deliver([existing, channel]); await completed;
    expect(cache.getQueryData(key)).toEqual([existing, channel]);
    expect(read).toHaveBeenCalledTimes(1);
  } finally { deliver([]); stop(); cache.clear(); }
});

it('repairs both visible projections of a live thread choice without reading the row again', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const keys = [
    ['settings', 'changed-thread'],
    ['thread-parameters', 'changed-thread'],
    ['thread', 'changed-thread'],
    ['node', 'changed-thread'],
    ['settings', 'other-thread'],
  ];
  const reads = keys.map(() => vi.fn(() => Promise.resolve('fresh')));
  const stops = keys.map((queryKey, index) => {
    cache.setQueryData(queryKey, 'held');
    const queryFn = reads[index];
    if (queryFn === undefined) throw new Error('Each fixture key has a reader');
    return new QueryObserver(cache, { queryKey, queryFn, staleTime: Infinity }).subscribe(() => undefined);
  });
  try {
    await applyChanges(threadRowChanges('changed-thread'), cache);
    expect(keys.map((key) => cache.getQueryData(key))).toEqual(['fresh', 'fresh', 'held', 'held', 'held']);
    expect(reads.map((read) => read.mock.calls.length)).toEqual([1, 1, 0, 0, 0]);
  } finally {
    stops.forEach((stop) => stop());
    cache.clear();
  }
});

it('applies the accepted document tabs to their captured document without reading either pane again', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const before: DocumentLinks = { shown: ['related'], own: null, inherited: ['related'], locked: false, writable: true };
  const accepted: DocumentLinks = { ...before, shown: ['references'], own: ['references'] };
  const edited = documentLinksQuery('edited-document'), other = documentLinksQuery('other-pane-document');
  const editedRead = vi.fn(() => Promise.resolve(before)), otherRead = vi.fn(() => Promise.resolve(before));
  cache.setQueryData(edited.queryKey, before); cache.setQueryData(other.queryKey, before);
  const stops = [
    new QueryObserver(cache, { ...edited, queryFn: editedRead, staleTime: Infinity }).subscribe(() => undefined),
    new QueryObserver(cache, { ...other, queryFn: otherRead, staleTime: Infinity }).subscribe(() => undefined),
  ];
  try {
    const act = findAct('PUT', '/documents/edited-document/links', { answer: accepted });
    expect(act).not.toBeNull();
    await applyChanges(act?.changes ?? [], cache);
    expect(cache.getQueryData(edited.queryKey)).toEqual(accepted);
    expect(cache.getQueryData(other.queryKey)).toEqual(before);
    expect(editedRead).not.toHaveBeenCalled(); expect(otherRead).not.toHaveBeenCalled();
  } finally { stops.forEach((stop) => stop()); cache.clear(); }
});

it.each([
  { method: 'PUT', path: '/documents/summary-document/summary-options', options: true, kind: 'document', range: undefined },
  { method: 'POST', path: '/summaries/document/summary-document/refresh', options: false, kind: 'document', range: undefined },
  { method: 'POST', path: '/summaries/chat/summary-document/refresh', options: false, kind: 'chat', range: { after: 10, through: 20 } },
])('repairs only the selected summary views after $method $path, without projecting its receipt', async ({ method, path, options, kind, range }) => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const id = 'summary-document';
  const entries = [
    { key: documentSummaryOptionsKey(id), changed: options },
    { key: resourceSummaryKey({ kind: 'document', id }), changed: kind === 'document' },
    { key: resourceSummaryKey({ kind: 'chat', id }, { after: 10, through: 20 }), changed: kind === 'chat' },
    { key: resourceSummaryKey({ kind: 'chat', id }, { after: 20, through: 30 }), changed: false },
    { key: resourceSummaryKey({ kind: 'chat', id }), changed: false },
    { key: resourceSummaryKey({ kind: 'document', id: 'other-document' }), changed: false },
    { key: documentSummaryOptionsKey('other-document'), changed: false },
    // The inventory counter shares this head, but is not a subject summary.
    { key: ['resource-summary', id] as const, changed: false },
  ];
  const reads = entries.map((_, index) => vi.fn(() => Promise.resolve({ view: `fresh-${index}` })));
  const stops = entries.map(({ key }, index) => {
    const read = reads[index]; if (read === undefined) throw new Error('A summary view has its read');
    cache.setQueryData(key, { view: `old-${index}` });
    return new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity }).subscribe(() => undefined);
  });
  const receipt = { projection: 'receipt', target: { kind, id }, revision: 9 };
  try {
    const act = findAct(method, path, { answer: receipt, input: range === undefined ? {} : { range } });
    expect(act).not.toBeNull(); await applyChanges(act?.changes ?? [], cache);
    entries.forEach(({ key, changed }, index) => {
      expect(reads[index], JSON.stringify(key)).toHaveBeenCalledTimes(changed ? 1 : 0);
      expect(cache.getQueryData(key)).toEqual({ view: `${changed ? 'fresh' : 'old'}-${index}` });
      expect(cache.getQueryData(key)).not.toEqual(receipt);
    });
  } finally { stops.forEach((stop) => stop()); cache.clear(); }
});

it.each([
  { name: 'without a returned projection', payload: {} },
  { name: 'with the native accepted destination', payload: { answer: { org: org('team', 'team', 'Team', userRef(me(1, 'owner', 'Owner'))), node: null, chat: null } } },
])('repairs every visible membership projection once $name, retires hidden ones and leaves unrelated views alone', async ({ payload }) => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const projections = [
    { key: ['org', 'team'], old: { id: 'org', member_count: 3 }, fresh: { id: 'org', member_count: 4 } },
    { key: ['members', 'org', { direct: false, q: '' }], old: ['before'], fresh: ['before', 'joined'] },
    { key: ['invitations', 'org'], old: [{ uses: 0 }], fresh: [{ uses: 1 }] },
  ];
  const reads = projections.map(({ fresh }) => vi.fn(() => Promise.resolve(fresh)));
  const stops: (() => void)[] = [];
  projections.forEach(({ key, old }, index) => {
    cache.setQueryData(key, old);
    const read = reads[index];
    if (read === undefined) throw new Error('missing projection reader');
    const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
    stops.push(observer.subscribe(() => undefined));
  });
  cache.setQueryData(['members', 'hidden'], ['old hidden view']);
  const unrelated = vi.fn(() => Promise.resolve('unchanged'));
  cache.setQueryData(['unrelated'], 'unchanged');
  stops.push(new QueryObserver(cache, { queryKey: ['unrelated'], queryFn: unrelated, staleTime: Infinity }).subscribe(() => undefined));
  try {
    await applyChanges(findAct('POST', '/invitations/accept', payload)?.changes ?? [], cache);
    projections.forEach(({ key, fresh }, index) => {
      expect(reads[index]).toHaveBeenCalledOnce();
      expect(cache.getQueryData(key)).toEqual(fresh);
    });
    expect(cache.getQueryData(['members', 'hidden'])).toBeUndefined();
    expect(unrelated).not.toHaveBeenCalled();
  } finally { stops.forEach((stop) => stop()); cache.clear(); }
});

it('retains a service-account projection outside the membership repair owner', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const entries = [
    { queryKey: ['members', 'team'], queryFn: vi.fn(() => Promise.resolve(['member', 'service-account'])) },
    { queryKey: ['service-accounts', 'team'], queryFn: vi.fn(() => Promise.resolve(['service-account'])) },
  ];
  const stops = entries.map((entry) => {
    cache.setQueryData(entry.queryKey, []);
    return new QueryObserver(cache, { ...entry, staleTime: Infinity }).subscribe(() => undefined);
  });
  try {
    await applyChanges(findAct('POST', '/orgs/team/service-accounts', { answer: { id: 'service-account', name: 'Account' } })?.changes ?? [], cache);
    expect(entries[0]?.queryFn).toHaveBeenCalledOnce();
    expect(entries[1]?.queryFn).toHaveBeenCalledOnce();
    expect(cache.getQueryData(['members', 'team'])).toEqual(['member', 'service-account']);
    expect(cache.getQueryData(['service-accounts', 'team'])).toEqual(['service-account']);
  } finally { stops.forEach((stop) => stop()); cache.clear(); }
});

const files = (directory: string): string[] =>
  readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const path = resolve(directory, entry.name);
    if (entry.isDirectory()) return entry.name === 'generated' ? [] : files(path);
    return /\.tsx?$/.test(entry.name) && !entry.name.endsWith('.d.ts') && entry.name !== 'routeTree.gen.ts' ? [path] : [];
  });

/** A helper is a source expression or the concrete outputs of a finite native path builder. */
type PathHelper = { expression: ts.Expression } | { paths: readonly string[] };
/** Resolve literals, templates, local helpers and both branches without exempting an unresolved request. */
function pathsOf(node: ts.Expression, helpers: ReadonlyMap<string, PathHelper>, depth = 0): string[] {
  if (depth > 4) return [ANY];
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) return [node.text];
  if (ts.isParenthesizedExpression(node) || ts.isAsExpression(node) || ts.isTypeAssertionExpression(node) || ts.isSatisfiesExpression(node) || ts.isNonNullExpression(node)) return pathsOf(node.expression, helpers, depth + 1);
  if (ts.isConditionalExpression(node)) return [...pathsOf(node.whenTrue, helpers, depth + 1), ...pathsOf(node.whenFalse, helpers, depth + 1)];
  if (ts.isCallExpression(node) && ts.isIdentifier(node.expression)) {
    const helper = helpers.get(node.expression.text);
    return helper === undefined ? [ANY] : 'paths' in helper ? [...helper.paths] : pathsOf(helper.expression, helpers, depth + 1);
  }
  if (ts.isTemplateExpression(node)) {
    let paths = [node.head.text];
    for (const span of node.templateSpans) {
      const parts = ts.isCallExpression(span.expression) || ts.isConditionalExpression(span.expression) ? pathsOf(span.expression, helpers, depth + 1) : [ANY];
      paths = paths.flatMap((path) => parts.map((part) => path + part + span.literal.text));
    }
    return paths;
  }
  return [ANY];
}

/** A path as the client would hand it to the registry: the template's `{name}` and the code's parameters as one segment each, without its query. */
const asked = (path: string): string => path.split('?')[0]!.replace(/\{\w+\}/g, ANY);

interface Request {
  where: string;
  method: string;
  path: string;
}

/** Every request in the sources that changes something: `api.POST(...)` and its kin, `requestJson('post', ...)`, and the trace's `traceJson(path, { method })`. */
function writeRequests(): Request[] {
  const found: Request[] = [];
  for (const file of files(sourceRoot)) {
    if (file.endsWith('/api/acts.ts')) continue;
    const source = ts.createSourceFile(file, readFileSync(file, 'utf8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
    const helpers = new Map<string, PathHelper>();
    const collect = (node: ts.Node) => {
      if (ts.isVariableDeclaration(node) && ts.isIdentifier(node.name) && node.initializer !== undefined && ts.isArrowFunction(node.initializer) && !ts.isBlock(node.initializer.body)) {
        helpers.set(node.name.text, { expression: node.initializer.body });
      }
      if (relative(sourceRoot, file) === 'api/moveOut.ts' && ts.isFunctionDeclaration(node) && node.name?.text === 'moveOutPath') {
        helpers.set(node.name.text, { paths: movePaths });
      }
      ts.forEachChild(node, collect);
    };
    collect(source);
    const where = (node: ts.Node) => `${relative(sourceRoot, file)}:${source.getLineAndCharacterOfPosition(node.getStart(source)).line + 1}`;
    const visit = (node: ts.Node) => {
      if (ts.isCallExpression(node)) {
        const callee = node.expression;
        const [first, second] = node.arguments;
        if (ts.isPropertyAccessExpression(callee) && ts.isIdentifier(callee.expression) && callee.expression.text === 'api' && WRITES.has(callee.name.text) && first !== undefined) {
          for (const path of pathsOf(first, helpers)) found.push({ where: where(node), method: callee.name.text, path });
        }
        if (ts.isIdentifier(callee) && callee.text === 'requestJson' && first !== undefined && ts.isStringLiteral(first) && second !== undefined && WRITES.has(first.text.toUpperCase())) {
          for (const path of pathsOf(second, helpers)) found.push({ where: where(node), method: first.text.toUpperCase(), path });
        }
        if (ts.isIdentifier(callee) && (callee.text === 'traceJson' || callee.text === 'traceRequest') && first !== undefined && second !== undefined && ts.isObjectLiteralExpression(second)) {
          const method = second.properties.find((property) => ts.isPropertyAssignment(property) && property.name.getText(source) === 'method');
          if (method !== undefined && ts.isPropertyAssignment(method) && ts.isStringLiteral(method.initializer) && WRITES.has(method.initializer.text)) {
            for (const path of pathsOf(first, helpers)) found.push({ where: where(node), method: method.initializer.text, path });
          }
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(source);
  }
  return found;
}

/** Every key every entry and every handler of the live connection can name, with each parameter standing for one thing. */
function namedHeads(): Set<string> {
  const heads = new Set<string>();
  const add = (changes: readonly Change[]) => {
    for (const change of changes) if ('key' in change) heads.add(change.key[0]);
  };
  for (const entry of Object.values(ACTS)) add(entry(() => ANY));
  for (const what of ['approvals', 'route', 'notification_prefs', 'notifications_read', 'favorites', 'state', 'org_creation', 'machines', 'resources', 'connected']) {
    add(userEventChanges(what, { machine_id: ANY, kind: 'provider', id: ANY }));
  }
  for (const kind of ['provider', 'agent', 'host', 'template', 'mcp_server', 'mount', 'environment', 'machine']) add(resourceChanges(kind, ANY));
  return heads;
}

describe('the registry of acts', () => {
  it('repairs a cancelled schedule without removing its history or reading another schedule or topic', async () => {
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const before = inputSchedule();
    const topicId = before.topic?.id;
    if (topicId === undefined) throw new Error('The schedule fixture needs a readable topic.');
    const after = inputSchedule({ state: 'deleted', next_run_at: null, revision: before.revision + 1 });
    const other = inputSchedule({ id: 'other-schedule' });
    const history = { pages: [{ items: [], next_cursor: null }], pageParams: [null] };
    const selected = inputScheduleQuery(before.id).queryKey;
    const otherKey = inputScheduleQuery(other.id).queryKey;
    const topic = topicSchedulesQuery(topicId).queryKey;
    const runs = inputScheduleRunsQuery(before.id).queryKey;
    const selectedRead = vi.fn(() => Promise.resolve(after));
    const otherRead = vi.fn(() => Promise.resolve(other));
    const topicRead = vi.fn(() => Promise.resolve([after]));
    const runsRead = vi.fn(() => Promise.resolve(history));
    const entries = [
      { key: selected, data: before, read: selectedRead },
      { key: otherKey, data: other, read: otherRead },
      { key: topic, data: [before], read: topicRead },
      { key: runs, data: history, read: runsRead },
    ];
    for (const entry of entries) cache.setQueryData(entry.key, entry.data);
    const stops = entries.map((entry) => new QueryObserver<unknown>(cache, {
      queryKey: entry.key, queryFn: () => entry.read(), staleTime: Infinity,
    }).subscribe(() => undefined));
    try {
      const act = findAct('DELETE', `/input-schedules/${before.id}`);
      expect(act).not.toBeNull();
      await applyChanges(act?.changes ?? [], cache);
      expect(selectedRead).toHaveBeenCalledOnce();
      expect(cache.getQueryData(selected)).toEqual(after);
      expect(otherRead).not.toHaveBeenCalled();
      expect(topicRead).not.toHaveBeenCalled();
      expect(runsRead).not.toHaveBeenCalled();
      expect(cache.getQueryData(otherKey)).toEqual(other);
      expect(cache.getQueryData(topic)).toEqual([before]);
      expect(cache.getQueryData(runs)).toEqual(history);
    } finally {
      stops.forEach((stop) => stop());
      cache.clear();
    }
  });

  it.each(['messages', 'stop', 'keep'])('repairs the changed Ask conversation and its visible history independently after %s', async (operation) => {
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const here = ['ask', 'viewer', 1, 'conversation', false], other = ['ask', 'viewer', 1, 'other', false];
    const history = ['ask-history', 'viewer', 1, 'root', false];
    const changed = vi.fn(() => Promise.resolve({ seq: 2 })), unrelated = vi.fn(() => Promise.resolve({ seq: 1 })), listed = vi.fn(() => Promise.resolve({ pages: ['fresh history'] }));
    const stops = [{ key: here, read: changed }, { key: other, read: unrelated }, { key: history, read: listed }].map(({ key, read }) => {
      cache.setQueryData(key, { seq: 1 });
      return new QueryObserver(cache, { queryKey: key, queryFn: async (): Promise<unknown> => read(), staleTime: Infinity }).subscribe(() => undefined);
    });
    try {
      const found = findAct('POST', `/ask/conversations/conversation/${operation}`, { answer: { conversation_id: 'conversation', seq: 2, turn_id: null } });
      expect(found).not.toBeNull(); await applyChanges(found?.changes ?? [], cache);
      expect(changed).toHaveBeenCalledOnce(); expect(listed).toHaveBeenCalledOnce(); expect(unrelated).not.toHaveBeenCalled();
      expect(cache.getQueryData(here)).toEqual({ seq: 2 }); expect(cache.getQueryData(other)).toEqual({ seq: 1 });
    } finally { stops.forEach((stop) => stop()); cache.clear(); }
  });

  it('registers Ask creation as a missing history projection without inventing a conversation snapshot from its receipt', () => {
    expect(findAct('POST', '/ask/conversations', { answer: { conversation_id: 'new-conversation', seq: 1, turn_id: null } })?.changes)
      .toEqual([{ missing: [{ key: ['ask-history'] }], cancelPending: true }]);
  });

  it('has an entry for every request of the application that changes something', () => {
    const requests = writeRequests();
    // The two requests of the session are the client's own and change nothing a screen reads by themselves.
    const acts = requests.filter((request) => !['/auth/session', '/auth/refresh', '/auth/logout'].includes(request.path));
    expect(acts.length).toBeGreaterThan(300);
    const missing = acts.filter((request) => findAct(request.method, asked(request.path)) === null).map((request) => `${request.where}: ${request.method} ${request.path}`);
    expect(missing).toEqual([]);
  }, SCAN_BUDGET_MS);

  it('names only data that some view of the application reads', () => {
    const sources = files(sourceRoot)
      .filter((file) => !file.endsWith('/api/acts.ts'))
      .map((file) => readFileSync(file, 'utf8'))
      .join('\n');
    const unread = [...namedHeads()].filter((head) => !sources.includes(`['${head}'`));
    expect(unread).toEqual([]);
  }, SCAN_BUDGET_MS);

  it('takes the most particular entry of a path, and reads its parameters', () => {
    expect(findAct('POST', '/machines/enrollment-tokens')?.act).toBe('POST /machines/enrollment-tokens');
    expect(findAct('POST', '/machines/m1/probe')?.act).toBe('POST /machines/{id}/probe');
    expect(findAct('DELETE', '/me/notification-routes/by-id/r1')?.act).toBe('DELETE /me/notification-routes/by-id/{route_id}');
    expect(findAct('DELETE', '/me/notification-routes/c1')?.act).toBe('DELETE /me/notification-routes/{channel_id}');
    expect(findAct('PATCH', '/documents/d1/permissions')?.act).toBe('PATCH /{object}/{id}/permissions');
    expect(findAct('GET', '/nodes/n1')).toBeNull();
    const roles = findAct('POST', '/nodes/n%201/roles');
    expect(roles?.act).toBe('POST /nodes/{id}/roles');
    const settings = findAct('PUT', '/nodes/t1/settings/model.default');
    expect(settings?.changes).toHaveLength(1);
    const change = settings?.changes[0];
    if (change === undefined || !('missing' in change)) throw new Error('a setting without a native projection names its missing view');
    expect(change.missing).toContainEqual({key:['node'],id:'t1'});
  });

  it('never marks a whole kind in any entry when the successful answer carries a row', () => {
    for (const entry of Object.keys(ACTS)) {
      const space = entry.indexOf(' ');
      const changes = findAct(entry.slice(0, space), asked(entry.slice(space + 1)), { answer: { id: 'x', name: 'The returned row' } })?.changes ?? [];
      expect(changes.filter((change) => 'key' in change && change.exact !== true), entry).toEqual([]);
      expect(changes.filter((change) => 'feed' in change), entry).toEqual([]);
    }
  });

  it('repairs the actual visible permission key once without reading another place or its lists', async () => {
    const cache = new QueryClient();
    const other = vi.fn(() => Promise.resolve({ name: 'Other place' }));
    const here = vi.fn(() => Promise.resolve({ name: 'Changed permission page' }));
    const provider = vi.fn(() => Promise.resolve([]));
    const entries = [
      { queryKey: ['page', 'nodes', 'thread-under-it'], queryFn: other },
      { queryKey: ['page', 'nodes', 'topic-1'], queryFn: here },
      { queryKey: ['provider-models', 'p1'], queryFn: provider },
    ];
    for (const entry of entries) cache.setQueryData(entry.queryKey, {});
    const observers = entries.map((entry) => new QueryObserver(cache, { ...entry, queryFn: (): Promise<unknown> => entry.queryFn(), staleTime: Infinity }));
    const stop = observers.map((observer) => observer.subscribe(() => undefined));
    try {
      await applyChanges(findAct('PATCH', '/nodes/topic-1/permissions')?.changes ?? [], cache);
      expect(here).toHaveBeenCalledTimes(1);
      expect(other).not.toHaveBeenCalled();
      expect(provider).not.toHaveBeenCalled();
      expect(cache.getQueryData(['page', 'nodes', 'topic-1'])).toEqual({ name: 'Changed permission page' });
      expect(cache.getQueryData(['page', 'topic-1'])).toBeUndefined();
    } finally {
      stop.forEach((off) => off());
      cache.clear();
    }
  });

  it('leaves browser, display and port lists to their live feeds after a restart', async () => {
    const cache = new QueryClient();
    const lists = (id: string): QueryKey[] => [browsersQuery(id).queryKey, displaysQuery(id).queryKey, portsKey(id)];
    const here = lists('environment-1');
    const elsewhere = lists('environment-2');
    for (const key of [...here, ...elsewhere]) cache.setQueryData(key, []);

    await applyChanges(findAct('POST', '/environments/environment-1/restart')?.changes ?? [], cache);

    for (const key of here) expect(cache.getQueryState(key)?.isInvalidated).toBe(false);
    for (const key of elsewhere) expect(cache.getQueryState(key)?.isInvalidated).toBe(false);
    for (const key of [...here, ...elsewhere]) expect(cache.getQueryData(key)).toEqual([]);
    cache.clear();
  });
});

describe('a change', () => {
  it('covers the queries that begin with its key and name its identifier anywhere in them', () => {
    expect(covers({ key: ['page'], id: 'a' }, ['page', 'nodes', 'a'])).toBe(true);
    expect(covers({ key: ['page'], id: 'a' }, ['page', 'nodes', 'b'])).toBe(false);
    expect(covers({ key: ['members'], id: 'o' }, ['members', { org: 'o', q: '' }])).toBe(true);
    expect(covers({ key: ['chat'], id: 'c', exact: true }, ['chat', 'c'])).toBe(false);
    expect(covers({ key: ['chat', 'c'], exact: true }, ['chat', 'c'])).toBe(true);
    expect(covers({ key: ['chat', 'c'], exact: true }, ['chat', 'c', 'blob', 'h'])).toBe(false);
    expect(covers({ key: ['tree-places'] }, ['tree'])).toBe(false);
  });

  it('drops what is gone and deduplicates an explicit feed repair, without staling inactive projections', async () => {
    const cache = new QueryClient();
    cache.setQueryData(['page', 'nodes', 'a'], 1);
    cache.setQueryData(['page', 'nodes', 'b'], 1);
    cache.setQueryData(['node', 'gone'], 1);
    cache.setQueryData(['provider', 'p'], 1);
    const asked: string[] = [];
    followWith({ trees: () => asked.push('trees'), thread: (id) => asked.push(`thread:${id}`), document: (id) => asked.push(`document:${id}`), counters: () => asked.push('counters') });
    await applyChanges([{ key: ['page'], id: 'a' }, { key: ['node'], id: 'gone', gone: true }, { feed: 'trees' }, { feed: 'trees' }, { feed: 'thread', id: 't' }], cache);
    expect(cache.getQueryState(['page', 'nodes', 'a'])?.isInvalidated).toBe(false);
    expect(cache.getQueryState(['page', 'nodes', 'b'])?.isInvalidated).toBe(false);
    expect(cache.getQueryState(['provider', 'p'])?.isInvalidated).toBe(false);
    expect(cache.getQueryData(['node', 'gone'])).toBeUndefined();
    expect(asked).toEqual(['trees', 'thread:t']);
  });
});

const row = (id: string, parent: string | null, kind: TreeNode['kind'], seq: number, more: Partial<TreeNode> = {}): TreeNode => treeNode({ id, kind, parent_id: parent, address: kind === 'org' ? id : null, tree_seq: seq, topic: kind === 'topic' ? { status: 'open', tag_ids: [] } : null, ...more });

describe('what the live tree says', () => {
  const held = new Map<string, TreeNode>(
    [row('acme', null, 'org', 1), row('web', 'acme', 'project', 1), row('login', 'web', 'topic', 1), row('t1', 'login', 'thread', 1)].map((node) => [node.id, node]),
  );

  it('takes a row as new as what is held for nothing', () => {
    expect(treeChanges(held, [row('web', 'acme', 'project', 1)])).toEqual([]);
  });

  it('applies a rename as its summary row without reading its parent lists or permissions', () => {
    const changed = row('web', 'acme', 'project', 2, { name: 'Web app' });
    expect(treeChanges(held, [changed])).toEqual([{ row: { kind: 'node', id: 'web', value: changed, partial: true } }]);
  });

  it('does not infer changes to every descendant from an unchanged summary', () => {
    const changes = treeChanges(held, [row('login', 'web', 'topic', 2)]);
    expect(changes).toHaveLength(1);
    expect(changes.some((change) => 'key' in change)).toBe(false);
  });

  it('carries the thread summary itself instead of refetching the topic and project', () => {
    const changed = row('t1', 'login', 'thread', 2);
    expect(treeChanges(held, [changed])).toEqual([{ row: { kind: 'node', id: 't1', value: changed, partial: true } }]);
  });

  it('drops what was read of a place that was deleted', () => {
    const changes = treeChanges(held, [row('login', 'web', 'topic', 3, { deleted: true })]);
    expect(changes).toContainEqual({ row: { kind: 'node', id: 'login', value: null, partial: true } });
  });
});

describe('the client tells of every act that succeeded', () => {
  const answer = (status: number) => new Response(status === 204 ? null : '{}', { status, headers: { 'Content-Type': 'application/json' } });

  it('before the act resolves, with its path below the base, and of nothing that failed or only read', async () => {
    let status = 200;
    const client = createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: () => Promise.resolve(answer(status)) });
    const told: string[] = [];
    let resolved = false;
    client.onAct(async (act) => {
      // The listener is waited for: the act has not resolved while it works.
      await Promise.resolve();
      told.push(`${act.method} ${act.path} ${resolved ? 'after' : 'before'}`);
    });
    await client.api.PATCH('/nodes/{id}', { params: { path: { id: 'n1' } }, body: { name: 'Web', expected_revision: 1 } }).then(() => {
      resolved = true;
    });
    await client.api.GET('/nodes/{id}', { params: { path: { id: 'n1' } } });
    status = 409;
    await client.api.DELETE('/roles/{id}', { params: { path: { id: 'r1' } } });
    expect(told).toEqual(['PATCH /nodes/n1 before']);
  });

  it('goes on when a listener fails: the server made the act', async () => {
    const client = createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: () => Promise.resolve(answer(204)) });
    const quiet = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    client.onAct(() => {
      throw new Error('a view could not be asked again');
    });
    const result = await client.api.DELETE('/roles/{id}', { params: { path: { id: 'r1' } } });
    expect(result.response.status).toBe(204);
    expect(quiet).toHaveBeenCalled();
  });
});

describe('what the person is told', () => {
  it.each([
    ['POST', '/me/push-subscriptions'],
    ['DELETE', '/me/push-subscriptions'],
    ['DELETE', '/me/push-subscriptions/device'],
  ])('refreshes the device roster after %s %s while preference data is also visible', async (method, path) => {
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const prefs = vi.fn(() => Promise.resolve({ revision: 2 }));
    const devices = vi.fn(() => Promise.resolve([{ id: 'device' }]));
    cache.setQueryData(['notification-prefs'], { revision: 1 });
    cache.setQueryData(['away', 'devices'], []);
    const stopPrefs = new QueryObserver(cache, { queryKey: ['notification-prefs'], queryFn: prefs, staleTime: Infinity }).subscribe(() => undefined);
    const stopDevices = new QueryObserver(cache, { queryKey: ['away', 'devices'], queryFn: devices, staleTime: Infinity }).subscribe(() => undefined);
    try {
      await applyChanges(findAct(method, path)?.changes ?? [], cache);
      expect(devices).toHaveBeenCalledTimes(1);
      expect(prefs).not.toHaveBeenCalled();
      expect(cache.getQueryData(['away', 'devices'])).toEqual([{ id: 'device' }]);
    } finally { stopPrefs(); stopDevices(); cache.clear(); }
  });

  it.each([
    { trigger: 'preference write', changes: () => findAct('PUT', '/me/notification-prefs', { answer: { revision: 2, pause_until: '2099-01-01T12:30:00Z' } })?.changes ?? [] },
    { trigger: 'route write', changes: () => findAct('PUT', '/me/notification-routes/mail', { answer: { id: 'route', channel_id: 'mail', enabled: true } })?.changes ?? [] },
    { trigger: 'preference hint', changes: () => userEventChanges('notification_prefs', {}) },
    { trigger: 'route hint', changes: () => userEventChanges('route', { channel_id: 'mail' }) },
    { trigger: 'preference snapshot', changes: () => userEventChanges('notification_prefs', { notification_prefs: { revision: 2, pause_until: '2099-01-01T12:30:00Z' } }) },
  ])('repairs visible outward status after $trigger without reading hidden places or devices', async ({ changes }) => {
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const entries = [
      { key: ['away', 'outside-status', 'thread'] as const, active: true, changed: true },
      { key: ['away', 'outside-status', 'org'] as const, active: true, changed: true },
      { key: ['away', 'outside-status', 'closed'] as const, active: false, changed: false },
      { key: ['away', 'devices'] as const, active: true, changed: false },
    ];
    const reads = entries.map((_, index) => vi.fn(() => Promise.resolve({ observed: `fresh-${index}` })));
    const stops = entries.flatMap(({ key, active }, index) => {
      cache.setQueryData(key, { observed: `old-${index}` });
      return active ? [new QueryObserver(cache, { queryKey: key, queryFn: reads[index], staleTime: Infinity }).subscribe(() => undefined)] : [];
    });
    try {
      await applyChanges(changes(), cache);
      entries.forEach(({ key, active, changed }, index) => {
        expect(reads[index], JSON.stringify(key)).toHaveBeenCalledTimes(changed ? 1 : 0);
        expect(cache.getQueryData(key)).toEqual(!active ? undefined : { observed: `${changed ? 'fresh' : 'old'}-${index}` });
      });
    } finally { stops.forEach((stop) => stop()); cache.clear(); }
  });

  it('carries a decided approval row, and does not treat a reconnect as a list change', () => {
    const approval = { id: 'a1', state: 'approved' };
    expect(userEventChanges('approvals', { approval })).toEqual([{ row: { kind: 'approval', id: 'a1', value: approval, insert: true } }]);
    expect(userEventChanges('connected', {})).toEqual([]);
  });

  it('names one resource for the news of one, and its kind for news without one', () => {
    expect(userEventChanges('resources', { kind: 'host', id: 'h1' })).toEqual([{ repair: { kind: 'host', id: 'h1', source: 'news' } }]);
    expect(userEventChanges('resources', { kind: 'host' })).toEqual([]);
    expect(userEventChanges('unknown', {})).toEqual([]);
  });
});

it('applies program acknowledgements and live machine registry changes without another read or inferred management',async()=>{
  const cache=new QueryClient();
  const key=['machine-programs','machine'];
  cache.setQueryData(key,{entries:[],capabilities:[],config_file:null,can_manage:true});
  const answer:MachinePrograms={entries:[{kind:'agent',name:'new-agent',command:['new-agent'],origin:'orch',ready:true,version:'1.0.0',
    descriptor:{kind:'new-agent',harness:{name:'New agent'},sandbox_modes:['open'],approval_modes:['on_request']},models:null,detail:null}],
    capabilities:[],config_file:'/data/config.toml',can_manage:true};
  const acknowledged=findAct('POST','/machines/machine/programs',{answer,input:{action:'reload'}});
  expect(acknowledged?.changes).toEqual([{value:{key,data:answer}}]);
  await applyChanges(acknowledged?.changes ?? [],cache);
  expect(cache.getQueryData(key)).toEqual(answer);
  const report:MachinePrograms={...answer,entries:answer.entries.map((entry)=>({...entry,ready:false,detail:'The program exited.'}))};
  await applyChanges([{row:{kind:'machine',id:'machine',value:{id:'machine',access:{permissions:['machine.view']},facts:{programs:report}}}}],cache);
  expect(cache.getQueryData(key)).toEqual({...report,can_manage:false});
  await applyChanges([{row:{kind:'machine',id:'machine',value:null}}],cache);
  expect(cache.getQueryData(key)).toBeUndefined();
});

it('applies the native recovery reply to every reader without another recovery request',async()=>{
  const cache=new QueryClient({defaultOptions:{queries:{retry:false}}});
  const key=['call-recovery','work'];
  const before={recovery:{state:'scheduled'}};
  const after={recovery:{state:'stopped',detail:'stopped_by_person'}};
  const read=vi.fn(()=>Promise.resolve(before));
  cache.setQueryData(key,before);
  const one=new QueryObserver(cache,{queryKey:key,queryFn:read,staleTime:Infinity});
  const two=new QueryObserver(cache,{queryKey:key,queryFn:read,staleTime:Infinity});
  const stopOne=one.subscribe(()=>undefined),stopTwo=two.subscribe(()=>undefined);
  try {
    await applyChanges(findAct('POST','/threads/work/calls/recovery',{answer:after})?.changes??[],cache);
    expect(one.getCurrentResult().data).toEqual(after);
    expect(two.getCurrentResult().data).toEqual(after);
    expect(read).not.toHaveBeenCalled();
  } finally {stopOne();stopTwo();cache.clear();}
});
