/** What each item of a thread is to the reader (conversation, work or diagnostics), and what that leaves of a new thread's first turn in each depth. */
import { describe, expect, it } from 'vitest';

import type { AgentRef, Item, MarkerPayload, ProviderRef } from '@/generated/api-types';
import { rowKind, type RowKind } from '@/features/timeline/rowKinds';
import { buildRows, type Row, type RowOptions } from '@/features/timeline/rows';
import { EMPTY_VIEW, viewFromRows } from '@/realtime/stores/thread';

import { me, userRef } from '../doubles/fixtures';
import { agentMessage, announcement, approvalItem, command, errorItem, marker, plan, reasoning, turn, userMessage } from '../doubles/thread-rows';

const ADA = userRef(me(1, 'ada', 'Ada Lovelace'));
const AT = '2026-10-01T10:00:00.000Z';
const AGENT = { id: 'agent-1', name: 'Codex', address: 'acme.codex', disabled: false, harness: 'codex', harness_kind: 'codex', machine: null } as unknown as AgentRef;
const PROVIDER = { id: 'provider-1', name: 'OpenAI-API-R4', address: 'acme.openai', harness: null, harness_kind: null, kind: 'api', placement: 'managed', status: 'ready' } as unknown as ProviderRef;

/** A report of the harness, as the adapters make it. */
function report(change: string, value: unknown, more: Record<string, unknown> = {}, id = `report-${change}`): Item {
  return marker(id, { marker: 'harness_changed', data: { program: 'Codex', change, subject: null, previous: { state: 'unsupported' }, value,
    expected: { state: 'unsupported' }, changed: false, diverged: false, next_turn: false, turn_id: null, reported_at: AT, ...more } } as unknown as MarkerPayload);
}
const history = (id: string, payload: unknown): Item => marker(id, payload as MarkerPayload);
const created = (): Item => history('created', { marker: 'created', data: { by: ADA, agent: null, provider: null, environment: null, model: null, effort: null } });
const prepared = (): Item => history('prepared', { marker: 'source_prepared', data: { by: ADA, directory: 'repo', branch: null, head: null, job_id: 'job-1' } });
const started = (resumed = false, id = 'started'): Item => history(id, { marker: 'segment_started', data: { agent: AGENT, provider: PROVIDER, ordinal: 1, resumed, segment_id: 'segment-1' } });

describe('the kind of an item', () => {
  const table: [string, Item, RowKind][] = [
    // The conversation: what people and the agent say, what waits for a person, an error, and a contradiction.
    ['a message of a person', userMessage('u', 'hello', { author: ADA }), 'conversation'],
    ['an answer of the agent', agentMessage('a', 'ready'), 'conversation'],
    ['an announcement', announcement('n', { title: 'Maintenance', body: 'Tonight', from: ADA }), 'conversation'],
    ['an error', errorItem('e', 'The model was rerouted', { code: 'model_rerouted' }), 'conversation'],
    ['an approval that waits', approvalItem('p', 'approval-1'), 'conversation'],
    ['a thread created', created(), 'conversation'],
    ['a source prepared', prepared(), 'conversation'],
    ['an agent started', started(), 'conversation'],
    ['a thread renamed', history('renamed', { marker: 'renamed', data: { by: ADA, from: 'a', to: 'b' } }), 'conversation'],
    ['a setting that differs from the request', report('model', 'gpt-6', { diverged: true, next_turn: true }), 'conversation'],
    ['an MCP server that failed to start', report('mcp_startup', { status: 'failed', message: 'no such file' }, {}, 'mcp-failed'), 'conversation'],
    ['a hook that blocked the action', report('hook', { status: 'blocked' }), 'conversation'],
    ['a hook that failed', report('hook', { status: 'failed' }, {}, 'hook-failed'), 'conversation'],
    ['a rate limit that rejected requests', report('rate_limit', { status: 'rejected' }), 'conversation'],
    ['a rate limit warning', report('rate_limit', { status: 'allowed_warning' }, {}, 'rate-warning'), 'conversation'],
    ['managed launch settings that differ', report('managed_settings_readback', { status: 'mismatch' }), 'conversation'],
    ['an event that says the session lost its writes', report('system_event', { kind: 'session_writes_lost' }), 'conversation'],
    ['an event that says a response was refused', report('system_event', { kind: 'model_refusal_no_fallback' }, {}, 'refused'), 'conversation'],
    // The work: what the agent did in a turn.
    ['reasoning', reasoning('r', 'Thinking'), 'work'],
    ['a plan', plan('pl', []), 'work'],
    ['a command', command('c', { command: 'ls', output: '', exit: 0 }), 'work'],
    ['an approval that was decided', { ...approvalItem('decided', 'approval-2'), status: 'approved' }, 'work'],
    ['a conversation that was compacted', history('compacted', { marker: 'compacted', data: { duration_ms: null } }), 'work'],
    ['a compaction the harness reported', report('context_compaction', { duration_ms: null }), 'work'],
    ['the context given to the agent', history('context', { marker: 'orch_context', data: { model_input_id: 'input-1', purpose: 'input' } }), 'work'],
    // Diagnostics: the program's own state.
    ['the session status', report('session_status', { type: 'active', active_flags: [] }), 'diagnostics'],
    ['the session going idle', report('session_status', { type: 'idle', active_flags: [] }, {}, 'idle'), 'diagnostics'],
    ['an MCP server starting', report('mcp_startup', { status: 'starting', message: null }, { subject: 'orch' }, 'mcp-starting'), 'diagnostics'],
    ['an MCP server ready', report('mcp_startup', { status: 'ready', message: null }, { subject: 'orch' }, 'mcp-ready'), 'diagnostics'],
    ['the collaboration mode', report('collaboration_mode', { mode: 'default', settings: { model: 'gpt-6-luna', reasoning_effort: 'medium' } }, { next_turn: true }), 'diagnostics'],
    ['the collaboration instructions', report('collaboration_instructions', null, { next_turn: true }), 'diagnostics'],
    ['the effort echoed back after a person chose it', report('effort', 'medium', { next_turn: true }), 'diagnostics'],
    ['the model echoed back after a person chose it', report('model', 'gpt-6-luna', { next_turn: true }, 'model-echo'), 'diagnostics'],
    ['the approval mode', report('approval_mode', { policy: 'never' }), 'diagnostics'],
    ['the sandbox mode', report('sandbox_mode', { type: 'workspaceWrite' }), 'diagnostics'],
    ['the environment connection', report('environment_connection', 'connected'), 'diagnostics'],
    ['reply buffering', report('reply_buffering', { show_ui: false }), 'diagnostics'],
    ['a native goal', report('native_goal', { status: 'active' }), 'diagnostics'],
    ['the session closing', report('session_lifecycle', 'closed'), 'diagnostics'],
    ['the managed launch settings as the program reports them', report('managed_settings_readback', { status: 'reported' }, {}, 'readback-ok'), 'diagnostics'],
    ['a managed setting that was read back', report('managed_setting', 'on', { subject: 'fast' }), 'diagnostics'],
    ['a hook that ran', report('hook', { status: 'completed' }, { subject: 'preToolUse' }, 'hook-ran'), 'diagnostics'],
    ['a notice of the program', report('system_event', { kind: 'notification', title: 'Indexed' }, {}, 'notice'), 'diagnostics'],
    ['a rate limit that allows requests', report('rate_limit', { status: 'allowed' }, {}, 'rate-ok'), 'diagnostics'],
    ['a program mode of a protocol agent', report('mode', 'code'), 'diagnostics'],
    ['the settings of a protocol agent', report('settings', { temperature: 0.2 }), 'diagnostics'],
    ['an event that is not recognised', report('unrecognized_event', { count: 1 }, { subject: 'warning' }), 'diagnostics'],
    ['the read state of a Codex thread, not recognised', report('unrecognized_event', { count: 1 }, { subject: 'thread/readState/changed' }, 'read-state'), 'diagnostics'],
  ];

  it.each(table)('is named for %s', (_label, item, kind) => {
    expect(rowKind(item)).toBe(kind);
  });
});

const options = (more: Partial<RowOptions> = {}): RowOptions => ({ detail: 'steps', hideHistory: false, foldOpened: new Set(), meId: null, ...more });

function viewOf(items: Item[], turns = [turn('t1', { ordinal: 1, state: 'completed', inputId: 'u-input' })]) {
  const numbered = items.map((item, index) => ({ ...item, seq_created: index + 1, seq_updated: index + 1 }));
  return viewFromRows(EMPTY_VIEW, { items: numbered, turns, inputs: [], thread: null }, false);
}

const ids = (rows: Row[]): string[] => rows.flatMap((row) => row.kind === 'item' ? [row.id] : row.kind === 'markers' || row.kind === 'start' ? [...row.ids] : []);

/** What the owner saw on a new thread with one "hello", in the order it was made. */
function firstTurn(): Item[] {
  return [
    created(), prepared(), started(),
    report('mcp_startup', { status: 'starting', message: null }, { subject: 'orch' }, 'mcp-starting'),
    report('mcp_startup', { status: 'ready', message: null }, { subject: 'orch' }, 'mcp-ready'),
    report('collaboration_mode', { mode: 'default', settings: { model: 'gpt-6-luna', reasoning_effort: 'medium' } }, { next_turn: true }),
    report('effort', 'medium', { next_turn: true }),
    report('collaboration_instructions', null, { next_turn: true }),
    report('session_status', { type: 'active', active_flags: [] }, {}, 'active'),
    report('unrecognized_event', { count: 1 }, { subject: 'warning' }),
    { ...userMessage('hello', 'hello', { author: ADA, turn: 't1' }) },
    agentMessage('answer', 'ready', { turn: 't1' }),
    report('unrecognized_event', { count: 1 }, { subject: 'thread/readState/changed', turn_id: 't1' }, 'read-state'),
    report('session_status', { type: 'idle', active_flags: [] }, { turn_id: 't1' }, 'idle'),
  ];
}

describe('the first turn of a new thread', () => {
  it.each(['summary', 'steps', 'everything'] as const)('is the start line, the message and the answer in %s detail', (detail) => {
    const rows = buildRows(viewOf(firstTurn()), options({ detail }));
    expect(rows.filter((row) => row.kind !== 'footer').map((row) => row.kind)).toEqual(['start', 'item', 'item']);
    expect(rows[0]).toMatchObject({ kind: 'start', ids: ['created', 'prepared', 'started'] });
    expect(ids(rows)).toEqual(['created', 'prepared', 'started', 'hello', 'answer']);
  });

  it('shows every line of it, the beginning one by one, when diagnostics are asked for', () => {
    const rows = buildRows(viewOf(firstTurn()), options({ diagnostics: true }));
    expect(rows.some((row) => row.kind === 'start')).toBe(false);
    expect(ids(rows)).toEqual(firstTurn().map((item) => item.id));
  });

  it('keeps its footer after the answer although the status lines that follow it are not drawn', () => {
    const rows = buildRows(viewOf(firstTurn()), options());
    expect(rows.at(-1)).toMatchObject({ kind: 'footer', turnId: 't1' });
  });

  it('stands as ordinary lines when no agent was started, and when the session was only resumed', () => {
    const lonely = buildRows(viewOf([created(), prepared()], []), options());
    expect(lonely.map((row) => row.kind)).toEqual(['markers']);
    expect(ids(lonely)).toEqual(['created', 'prepared']);
    const resumed = buildRows(viewOf([created(), started(true)], []), options());
    expect(resumed.some((row) => row.kind === 'start')).toBe(false);
  });

  it('leaves no start line where the history of the thread is hidden', () => {
    const rows = buildRows(viewOf(firstTurn()), options({ hideHistory: true }));
    expect(ids(rows)).toEqual(['hello', 'answer']);
  });
});

describe('an answer that is said twice', () => {
  const said = (first: Item, second: Item): string[] => ids(buildRows(viewOf([userMessage('hello', 'hello', { author: ADA, turn: 't1' }), first, second]), options()));

  it('stands once, as the answer, when a message that is not the answer is repeated by a later message of the turn', () => {
    expect(said(agentMessage('said', 'ready', { turn: 't1', phase: 'commentary' }), agentMessage('answer', 'ready', { turn: 't1', phase: 'final' }))).toEqual(['hello', 'answer']);
  });

  it('gives its mark and name to the message that stays', () => {
    const rows = buildRows(viewOf([userMessage('hello', 'hello', { author: ADA, turn: 't1' }),
      agentMessage('said', 'ready', { turn: 't1', phase: 'commentary' }), agentMessage('answer', 'ready', { turn: 't1', phase: 'final' })]), options());
    expect(rows.find((row) => row.kind === 'item' && row.id === 'answer')).toMatchObject({ head: true });
  });

  it('keeps both when the texts differ, or when they are of two turns', () => {
    expect(said(agentMessage('said', 'working on it', { turn: 't1', phase: 'commentary' }), agentMessage('answer', 'ready', { turn: 't1', phase: 'final' }))).toEqual(['hello', 'said', 'answer']);
    const turns = [turn('t1', { ordinal: 1, state: 'completed', inputId: 'u-input' }), turn('t2', { ordinal: 2, state: 'completed', inputId: 'u2-input' })];
    const rows = buildRows(viewOf([agentMessage('first', 'ok', { turn: 't1', phase: 'commentary' }), agentMessage('second', 'ok', { turn: 't2', phase: 'final' })], turns), options());
    expect(ids(rows)).toEqual(['first', 'second']);
  });

  it('never hides a message that was given as the answer', () => {
    expect(said(agentMessage('one', 'ready', { turn: 't1', phase: 'final' }), agentMessage('two', 'ready', { turn: 't1', phase: 'final' }))).toEqual(['hello', 'one', 'two']);
  });
});

describe('a report that contradicts what the person chose', () => {
  const diverged = report('effort', 'low', { diverged: true, next_turn: true, turn_id: 't1', expected: { state: 'told', value: 'high' } }, 'effort-differs');
  const notice = { ...errorItem('notice', 'Codex reported effort that differs from Orch\'s request for future turns.', { code: 'harness_diverged', turn: 't1' }),
    payload: { code: 'harness_diverged', message: 'Codex reported effort that differs from Orch\'s request for future turns.', retryable: false, details: { turn_id: 't1', change: 'effort', next_turn: true } } } as Item;
  const items = (): Item[] => [userMessage('hello', 'hello', { author: ADA, turn: 't1' }), diverged, notice, agentMessage('answer', 'ready', { turn: 't1' })];

  it('is told once, by the line of the report, and not again by the error the harness adds beside it', () => {
    expect(ids(buildRows(viewOf(items()), options()))).toEqual(['hello', 'effort-differs', 'answer']);
  });

  it('is told by both where diagnostics are asked for', () => {
    expect(ids(buildRows(viewOf(items()), options({ diagnostics: true })))).toEqual(['hello', 'effort-differs', 'notice', 'answer']);
  });

  it('leaves an error of the turn alone when no report stands beside it', () => {
    const lone = [userMessage('hello', 'hello', { author: ADA, turn: 't1' }), notice, agentMessage('answer', 'ready', { turn: 't1' })];
    expect(ids(buildRows(viewOf(lone), options()))).toEqual(['hello', 'notice', 'answer']);
  });
});
