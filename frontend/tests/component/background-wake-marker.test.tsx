import { act, cleanup, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it } from 'vitest';
import type { Item, MarkerBackgroundWake, ThreadInput } from '@/generated/api-types';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { MarkerLine } from '@/features/timeline/items/Marker';
import { buildRows } from '@/features/timeline/rows';
import { defaultThreadRead } from '@/realtime/threadScope';
import { threadViewOf, threadWriter, useThreadPart } from '@/realtime/stores/thread';
import { i18n, setLocale } from '@/i18n';
import { backgroundWake, wakeCause } from '../doubles/background-wake';
import { agentMessage, marker, threadInput, turn } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const THREAD = 'wake-thread';
const READ = defaultThreadRead(THREAD);
const author = userRef(me(3, 'alex', 'Alex'));
const context: TimelineContext = { threadId: THREAD, threadName: 'Repair login', detail: 'summary', recent: new Set(), layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: false, canManage: false, live: true, lit: null, selection: NOT_PICKING };
const data = (more: Partial<MarkerBackgroundWake> = {}): MarkerBackgroundWake => ({ admitted: true, input_id: 'wake-input', causes: [wakeCause()], ...more });
function numbered(item: Item, sequence: number): Item { return { ...item, thread_id: THREAD, segment_id: 'wake-segment', seq_created: sequence, seq_updated: sequence }; }
function wakeMarker(value = data()): Item { return numbered(marker('wake-marker', { marker: 'background_wake', data: value }, { turn: value.admitted ? 'wake-turn' : null }), 1); }
function LiveMarker() {
  const item = useThreadPart(THREAD, (view) => view.items.get('wake-marker'));
  return item?.kind === 'marker' ? <MarkerLine item={item} /> : null;
}
function load(value = data(), detail: string | null = null) {
  const thread = { ...snapshot().thread, id: THREAD, model: 'model-route', model_display: 'Review model', environment: null,
    background: backgroundWake({ tasks: [], held_completions: 2, waiting_author: author, waiting_running: 3 }) };
  const input: ThreadInput = { ...threadInput('wake-input', 'An internal continuation', { author, position: '1', source: 'system' }), thread_id: THREAD, state_detail: detail };
  threadWriter.reset(READ, { thread, inputs: [input], turns: value.admitted ? [turn('wake-turn', { ordinal: 2, state: 'running', inputId: input.id })] : [],
    items: value.admitted ? [wakeMarker(value), numbered(agentMessage('wake-output', 'The model claims a different cause.', { turn: 'wake-turn' }), 2)] : [wakeMarker(value)] }, false);
}

beforeEach(() => { threadWriter.evict(READ); load(); });
afterEach(async () => { cleanup(); threadWriter.evict(READ); await setLocale('en'); document.documentElement.removeAttribute('data-theme'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  it(`keeps the recorded multi-cause wake before output with history hidden at ${width}px ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    load(data({ causes: [wakeCause(), wakeCause({ id: 'auth-review', kind: 'subagent', label: 'Review auth', state: 'failed', exit_code: null })] }));
    const view = threadViewOf(READ);
    const rows = buildRows(view, { detail: 'summary', hideHistory: true, foldOpened: new Set(), meId: null });
    expect(rows.find((row) => row.kind === 'item')).toMatchObject({ id: 'wake-marker' });
    const { container } = show(<TimelineContextValue.Provider value={context}><div style={{ width }}>
      {rows.flatMap((row) => {
        if (row.kind !== 'item') return [];
        const item = view.items.get(row.id);
        return item?.kind === 'marker' ? [<MarkerLine key={item.id} item={item} />] : item?.kind === 'agent_message' ? [<p key={item.id} data-wake-output>{item.payload.text}</p>] : [];
      })}
    </div></TimelineContextValue.Provider>);
    const expected = locale === 'en' ? 'Woken by: Build web finished, Review auth failed' : '唤醒原因：Build web 已完成, Review auth 失败';
    expect(screen.getByText(expected)).toBeVisible();
    const wake = container.querySelector('[data-background-wake]');
    const output = container.querySelector('[data-wake-output]');
    if (wake === null || output === null) throw new Error('The continuation needs its cause and output');
    expect(wake.compareDocumentPosition(output) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
    expect(wake).not.toHaveTextContent('different cause');
  });
}

it.each(['model_use_required', 'author_lost_access', 'background_wake_off', 'background_stopped', 'background_refresh', 'background_author'] as const)('shows the ordinary %s hold with the recorded cause before admission', (detail) => {
  load(data({ admitted: false }), detail);
  const { container } = show(<TimelineContextValue.Provider value={context}><LiveMarker /></TimelineContextValue.Provider>);
  const wake = container.querySelector('[data-background-wake="held"]');
  expect(wake).toHaveTextContent('Build web finished');
  expect(wake).not.toHaveTextContent('Woken by:');
  if (detail === 'background_author') expect(wake).toHaveTextContent("Waiting for Alex's background tasks (3 running)");
  if (detail === 'background_stopped') expect(wake).toHaveTextContent('2 finished tasks waiting');
  if (detail === 'background_wake_off') expect(wake).toHaveTextContent(i18n.t('backgroundWake.wakeOffHold'));
  if (detail === 'author_lost_access') expect(wake).toHaveTextContent(i18n.t('backgroundWake.authorLostHold', { name: 'Alex' }));
});

it('changes a held line to Woken by only after the admitted marker update arrives', () => {
  load(data({ admitted: false }), 'model_use_required');
  show(<TimelineContextValue.Provider value={context}><LiveMarker /></TimelineContextValue.Provider>);
  expect(screen.queryByText('Woken by: Build web finished')).not.toBeInTheDocument();
  act(() => threadWriter.apply(READ, { turns: [turn('wake-turn', { ordinal: 2, state: 'running', inputId: 'wake-input' })], items: [{ ...wakeMarker(data()), seq_updated: 3 }] }));
  expect(screen.getByText('Woken by: Build web finished')).toBeVisible();
});
