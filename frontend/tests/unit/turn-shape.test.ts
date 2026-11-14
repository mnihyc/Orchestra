/** What a turn looks like in the conversation: the person's message, the work of the agent as one group, its answer, and what stands under it. */
import { describe, expect, it } from 'vitest';

import type { AgentRef, Item, MarkerPayload, Segment, Thread } from '@/generated/api-types';
import { changedAgent } from '@/features/timeline/Steps';
import { firstSummaryLine, thinkingLine } from '@/features/timeline/thinking';
import { buildRows, type Row, type RowOptions } from '@/features/timeline/rows';
import { applyRows, EMPTY_VIEW, type ThreadView, viewFromRows } from '@/realtime/stores/thread';

import { me, userRef } from '../doubles/fixtures';
import { agentMessage, approvalItem, change, command, errorItem, fileChange, marker, plan, reasoning, turn, userMessage } from '../doubles/thread-rows';

const ADA = userRef(me(1, 'ada', 'Ada Lovelace'));
const GRACE = userRef(me(2, 'grace', 'Grace Hopper'));
const AT = '2026-10-01T10:00:00.000Z';

const options = (more: Partial<RowOptions> = {}): RowOptions => ({ detail: 'normal', hideHistory: false, meId: null, ...more });

function viewOf(items: Item[], turns = [turn('t1', { ordinal: 1, state: 'completed', inputId: 'u-input' })]): ThreadView {
  const numbered = items.map((item, index) => ({ ...item, seq_created: index + 1, seq_updated: index + 1 }));
  return viewFromRows(EMPTY_VIEW, { items: numbered, turns, inputs: [], thread: null }, false);
}

const report = (change: string, value: unknown, id: string): Item => marker(id, { marker: 'harness_changed', data: { program: 'Codex', change, subject: null, previous: { state: 'unsupported' }, value,
  expected: { state: 'unsupported' }, changed: false, diverged: false, next_turn: false, turn_id: null, reported_at: AT } } as unknown as MarkerPayload);

const kinds = (rows: Row[]): string[] => rows.map((row) => row.kind);
const work = (rows: Row[]): Extract<Row, { kind: 'work' }>[] => rows.flatMap((row) => row.kind === 'work' ? [row] : []);
const said = (id: string, text: string, phase: 'commentary' | 'final', more: { status?: Item['status'] } = {}): Item =>
  agentMessage(id, text, { turn: 't1', phase, ...(more.status === undefined ? {} : { status: more.status }) });
/** What a program that marks nothing says. */
function unmarked(id: string, text: string): Item {
  const item = agentMessage(id, text, { turn: 't1' });
  return item.kind === 'agent_message' ? { ...item, payload: { ...item.payload, phase: null } } : item;
}

/** The second turn walked on a real thread: 93 items, of which 33 are sentences of the agent, 29 commands, 3 file changes, 26 lines of the program's own state, one context line and one message of the person. */
function longTurn(): Item[] {
  const items: Item[] = [userMessage('person', 'Make the package and run its tests', { author: ADA, turn: 't1' })];
  for (let step = 0; step < 29; step += 1) {
    items.push(said(`say-${step}`, `Step ${step}: working on it`, 'commentary'));
    items.push(command(`run-${step}`, { command: `step ${step}`, output: 'ok', exit: step === 3 ? 1 : 0, turn: 't1' }));
    if (step < 26) items.push(report('session_status', { type: 'active', active_flags: [] }, `state-${step}`));
  }
  for (let file = 0; file < 3; file += 1) items.push(fileChange(`edit-${file}`, { turn: 't1', changes: [change(`src/file-${file}.py`, '+one\n+two\n-three')] }));
  for (let extra = 29; extra < 32; extra += 1) items.push(said(`say-${extra}`, `Then ${extra}`, 'commentary'));
  items.push(marker('context', { marker: 'orch_context', data: { model_input_id: 'record', purpose: 'input' } }, { turn: 't1' }));
  items.push(said('answer', 'Done. The package is built.', 'final'));
  return items;
}

describe('a finished turn of ninety-three items', () => {
  it('draws as the person’s message, one line for the work, the answer and what stands under it', () => {
    const items = longTurn();
    expect(items).toHaveLength(93);
    const rows = buildRows(viewOf(items), options());
    expect(kinds(rows)).toEqual(['item', 'work', 'item', 'footer']);
    const group = work(rows)[0];
    expect(group).toMatchObject({ turnId: 't1', last: true, live: false, only: true });
    // The commands, the file changes and the sentences between them, and nothing else: the program's own state is not drawn.
    expect(group?.ids).toHaveLength(29 + 3 + 32);
    expect(rows[2]).toMatchObject({ kind: 'item', id: 'answer', answer: true });
    expect(rows[3]).toMatchObject({ kind: 'footer', turnId: 't1', answered: true, lined: true });
  });

  it('keeps the sentences between the steps inside the group and the program’s own state out of every reading but Diagnostics', () => {
    const ids = (rows: Row[]) => rows.flatMap((row) => row.kind === 'item' ? [row.id] : []);
    expect(ids(buildRows(viewOf(longTurn()), options()))).toEqual(['person', 'answer']);
    const verbose = buildRows(viewOf(longTurn()), options({ detail: 'verbose' }));
    expect(work(verbose)).toHaveLength(0);
    expect(ids(verbose)).toHaveLength(1 + 29 + 29 + 3 + 3 + 1);
    expect(ids(verbose)).not.toContain('state-0');
    const diagnostics = buildRows(viewOf(longTurn()), options({ detail: 'diagnostics' }));
    expect(diagnostics.flatMap((row) => row.kind === 'markers' ? [...row.ids] : [])).toContain('state-0');
  });
});

describe('the final answer of a turn', () => {
  const rowsOf = (items: Item[], state: 'completed' | 'interrupted' | 'failed' | 'running' = 'completed') =>
    buildRows(viewOf([userMessage('person', 'Go', { author: ADA, turn: 't1' }), ...items], [turn('t1', { ordinal: 1, state, inputId: 'u-input' })]), options());

  it('is the message the program marked final, and what it said before it is commentary of the work', () => {
    const rows = rowsOf([said('first', 'Let me look.', 'commentary'), said('last', 'Done.', 'final')]);
    expect(kinds(rows)).toEqual(['item', 'work', 'item', 'footer']);
    expect(work(rows)[0]?.ids).toEqual(['first']);
    expect(rows[2]).toMatchObject({ id: 'last', answer: true });
  });

  it('is the last words of a turn that nobody marked, once the turn is over', () => {
    const rows = rowsOf([command('c', { command: 'ls', output: '', exit: 0, turn: 't1' }), unmarked('words', 'All done')]);
    expect(kinds(rows)).toEqual(['item', 'work', 'item', 'footer']);
    expect(rows[2]).toMatchObject({ id: 'words', answer: true });
  });

  it('keeps marked commentary inside the work when a turn is stopped without a final answer', () => {
    const rows = rowsOf([command('c', { command: 'ls', output: '', exit: 0, turn: 't1' }), said('words', 'I will now write the file', 'commentary')], 'interrupted');
    expect(kinds(rows)).toEqual(['item', 'work', 'footer']);
    expect(work(rows)[0]?.ids).toEqual(['c', 'words']);
    expect(rows.at(-1)).not.toHaveProperty('answered');
  });

  it('uses only the last unmarked message as the answer and folds earlier trailing words into the work', () => {
    const rows = rowsOf([command('c', { command: 'ls', output: '', exit: 0, turn: 't1' }), unmarked('earlier', 'The listing is ready'), unmarked('answer', 'The files are ready')]);
    expect(kinds(rows)).toEqual(['item', 'work', 'item', 'footer']);
    expect(work(rows)[0]?.ids).toEqual(['c', 'earlier']);
    expect(rows[2]).toMatchObject({ id: 'answer', answer: true });
  });

  it('is not there when the turn ended with a step, and the footer carries the acts of the turn', () => {
    const rows = rowsOf([said('first', 'Looking', 'commentary'), command('c', { command: 'ls', output: '', exit: 0, turn: 't1' })], 'failed');
    expect(kinds(rows)).toEqual(['item', 'work', 'footer']);
    expect(work(rows)[0]?.ids).toEqual(['first', 'c']);
    expect(rows[2]).toMatchObject({ kind: 'footer', lined: true });
    expect(rows[2]).not.toHaveProperty('answered');
  });

  it('draws a turn with no work as the message and the answer, with no line between them', () => {
    const rows = rowsOf([said('answer', 'ready', 'final')]);
    expect(kinds(rows)).toEqual(['item', 'item', 'footer']);
    expect(rows[2]).toMatchObject({ kind: 'footer', answered: true });
    expect(rows[2]).not.toHaveProperty('lined');
  });

  it('is not marked while the turn runs: the text written after the latest step stands under the group as the reply in progress', () => {
    const items = [command('c', { command: 'ls', output: '', exit: 0, turn: 't1' }), said('reply', 'The listing shows', 'commentary', { status: 'in_progress' })];
    const rows = rowsOf(items, 'running');
    expect(kinds(rows)).toEqual(['item', 'work', 'item']);
    expect(work(rows)[0]).toMatchObject({ ids: ['c'], live: true, last: true });
    expect(rows[2]).toEqual({ kind: 'item', key: 'reply', id: 'reply' });
    // A step that follows takes the sentence into the group.
    const later = rowsOf([...items, command('d', { command: 'pwd', output: '', exit: null, turn: 't1' })], 'running');
    expect(kinds(later)).toEqual(['item', 'work']);
    expect(work(later)[0]?.ids).toEqual(['c', 'reply', 'd']);
  });
});

describe('what stands outside the group', () => {
  it('puts a question that waits for a person under the group it came from, whole, and makes it a step once it is decided', () => {
    const pending = approvalItem('ask', 'approval-1', { turn: 't1' });
    const running = [turn('t1', { ordinal: 1, state: 'running', inputId: 'u-input' })];
    const waiting = buildRows(viewOf([userMessage('person', 'Go', { author: ADA, turn: 't1' }), command('c', { command: 'ls', output: '', exit: null, turn: 't1' }), pending], running), options());
    expect(kinds(waiting)).toEqual(['item', 'work', 'item']);
    expect(waiting[2]).toMatchObject({ id: 'ask' });
    expect(work(waiting)[0]?.ids).toEqual(['c']);
    const decided = buildRows(viewOf([userMessage('person', 'Go', { author: ADA, turn: 't1' }), command('c', { command: 'ls', output: '', exit: 0, turn: 't1' }), { ...pending, status: 'approved' }], running), options());
    expect(kinds(decided)).toEqual(['item', 'work']);
    expect(work(decided)[0]?.ids).toEqual(['c', 'ask']);
  });

  it('keeps one group when a question waits among later steps of the same turn', () => {
    const rows = buildRows(viewOf([userMessage('person', 'Go', { author: ADA, turn: 't1' }), command('a', { command: 'ls', output: '', exit: 0, turn: 't1' }), approvalItem('ask', 'approval-1', { turn: 't1' }),
      command('b', { command: 'pwd', output: '', exit: 0, turn: 't1' })], [turn('t1', { ordinal: 1, state: 'running', inputId: 'u-input' })]), options());
    expect(kinds(rows)).toEqual(['item', 'work', 'item']);
    expect(work(rows)[0]?.ids).toEqual(['a', 'b']);
  });

  it('stands an error where it was made, between two groups', () => {
    const rows = buildRows(viewOf([userMessage('person', 'Go', { author: ADA, turn: 't1' }), command('a', { command: 'ls', output: '', exit: 0, turn: 't1' }),
      errorItem('oops', 'The model was rerouted', { code: 'model_rerouted', turn: 't1' }), command('b', { command: 'pwd', output: '', exit: 0, turn: 't1' })]), options());
    expect(kinds(rows)).toEqual(['item', 'work', 'item', 'work', 'footer']);
    expect(work(rows).map((group) => group.only)).toEqual([false, false]);
  });

  it('makes two groups of the work before and after a message steered into the running turn', () => {
    const steer = userMessage('steer', 'Use the other file', { author: ADA, turn: 't1', delivery: 'steer' });
    const rows = buildRows(viewOf([userMessage('person', 'Go', { author: ADA, turn: 't1' }), command('a', { command: 'ls', output: '', exit: 0, turn: 't1' }), steer,
      command('b', { command: 'pwd', output: '', exit: null, turn: 't1' })], [turn('t1', { ordinal: 1, state: 'running', inputId: 'u-input' })]), options());
    expect(kinds(rows)).toEqual(['item', 'work', 'item', 'work']);
    expect(work(rows).map((group) => ({ ids: group.ids, live: group.live, last: group.last }))).toEqual([{ ids: ['a'], live: false, last: false }, { ids: ['b'], live: true, last: true }]);
  });

  it('draws the plan and the thought as steps of the group', () => {
    const rows = buildRows(viewOf([userMessage('person', 'Go', { author: ADA, turn: 't1' }), reasoning('thought', 'Looking at the route', { turn: 't1' }), plan('plan', [{ text: 'Fix it', status: 'pending' }], { turn: 't1' }), said('answer', 'ok', 'final')]), options());
    expect(work(rows)[0]?.ids).toEqual(['thought', 'plan']);
  });
});

describe('the person’s message', () => {
  it('names its sender only where more than one person wrote in the thread', () => {
    const one = buildRows(viewOf([userMessage('a', 'Hello', { author: ADA, turn: 't1' }), userMessage('b', 'Again', { author: ADA, turn: 't1' })]), options());
    expect(one.every((row) => row.kind !== 'item' || row.named === undefined)).toBe(true);
    const two = buildRows(viewOf([userMessage('a', 'Hello', { author: ADA, turn: 't1' }), userMessage('b', 'Me too', { author: GRACE, turn: 't1' })]), options());
    expect(two.flatMap((row) => row.kind === 'item' ? [row.named === true] : [])).toEqual([true, true]);
  });
});

describe('the thought that is being written', () => {
  const think = (summary: string, status: Item['status'] = 'in_progress', after: Item[] = []): ThreadView => {
    const base = viewOf([userMessage('person', 'Go', { author: ADA, turn: 't1' }), { ...reasoning('thought', summary, { turn: 't1' }), status }, ...after], [turn('t1', { ordinal: 1, state: 'running', inputId: 'u-input' })]);
    return applyRows(base, { thread: { seq: 100, state: 'running', lock: null, my_last_read_seq: null, current_turn_id: 't1' } as unknown as Thread });
  };

  it('is its first line, without the marks of Markdown', () => {
    expect(thinkingLine(think('**Planning the approach**\n\nI should read the tests first.'))).toBe('Planning the approach');
    expect(firstSummaryLine('\n\n## Checking `core.py`\nmore')).toBe('Checking core.py');
    expect(firstSummaryLine('   ')).toBeNull();
  });

  it('is nothing once the thought is over or the agent has gone on to a step', () => {
    expect(thinkingLine(think('Planning', 'completed'))).toBeNull();
    expect(thinkingLine(think('Planning', 'in_progress', [command('c', { command: 'ls', output: '', exit: null, turn: 't1' })]))).toBeNull();
    expect(thinkingLine(viewOf([reasoning('thought', 'Planning', { turn: 't1' })]))).toBeNull();
  });
});

describe('which agent answered', () => {
  const agent = (id: string, name: string) => ({ id, name }) as unknown as AgentRef;
  const segment = (id: string, who: AgentRef | null) => ({ id, agent: who, ordinal: 1 }) as unknown as Segment;
  const viewWith = (turnSegments: string[], segments: Segment[]): ThreadView => viewFromRows(EMPTY_VIEW, {
    items: [], inputs: [], thread: null, segments,
    turns: turnSegments.map((segmentId, index) => ({ ...turn(`t${index + 1}`, { ordinal: index + 1, state: 'completed', inputId: `i${index + 1}` }), segment_id: segmentId })),
  }, false);

  it('is said only where it is not the agent of the turn before', () => {
    const view = viewWith(['s1', 's1', 's2'], [segment('s1', agent('a1', 'Codex')), segment('s2', agent('a2', 'Claude Code'))]);
    expect(changedAgent(view, view.turns.get('t1'))).toBeNull();
    expect(changedAgent(view, view.turns.get('t2'))).toBeNull();
    expect(changedAgent(view, view.turns.get('t3'))).toBe('Claude Code');
  });

  it('is not said where a segment has no agent of its own to name', () => {
    const view = viewWith(['s1', 's2'], [segment('s1', null), segment('s2', null)]);
    expect(changedAgent(view, view.turns.get('t2'))).toBeNull();
  });
});
