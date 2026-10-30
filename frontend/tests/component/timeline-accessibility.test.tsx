/** Tool sections and context links remain usable in either catalogue and theme. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { MarkerLine } from '@/features/timeline/items/Marker';
import { RawCall } from '@/features/timeline/RawCall';
import { McpDisplay } from '@/features/timeline/tools/Mcp';
import { TurnFooter } from '@/features/timeline/TurnFooter';
import { effortDisplayName } from '@/features/models/names';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { marker, toolCall, turn } from '../doubles/thread-rows';
import { show } from './support';

const given = vi.hoisted(() => vi.fn<(thread: string, selection: { turn?: string; record?: string }) => void>());
vi.mock('@/features/inspector/givenFocus', () => ({ openGiven: given }));
vi.mock('@/features/source/Restore', () => ({ RestoreFromTurn: () => null }));
const THREAD = 'accessibility-thread';
const context: TimelineContext = { threadId: THREAD, threadName: 'Login', detail: 'steps', recent: new Set(), layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: false, canManage: false, live: true, lit: null, selection: NOT_PICKING };
const stored = (state: 'completed' | 'running' = 'completed') => turn('turn-1', { ordinal: 1, state, inputId: 'input-1', completedAt: state === 'completed' ? '2026-10-01T10:00:01Z' : null });
const pointer = (id: string, record: string) => marker(id, { marker: 'orch_context', data: { model_input_id: record, purpose: 'input' } }, { turn: 'turn-1' });
let locale = currentLocale();
let theme: string | null = null;
beforeEach(() => { locale = currentLocale(); theme = document.documentElement.getAttribute('data-theme'); given.mockClear(); threadWriter.evict(THREAD); });
afterEach(async () => { threadWriter.evict(THREAD); await setLocale(locale);
  if (theme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', theme); });

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('timeline in %s / %s', (language, look) => {
  beforeEach(async () => { await setLocale(language); document.documentElement.setAttribute('data-theme', look); });
  it('omits the first and unchanged model and names only a later model change', () => {
    const first = stored();
    const same = { ...first, id: 'turn-2', ordinal: 2 };
    const changed = { ...first, id: 'turn-3', ordinal: 3, model: 'next-model', model_display: 'Next Model' };
    threadWriter.reset(THREAD, { turns: [first, same, changed] }, false);
    const view = show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={first.id} /></TimelineContextValue.Provider>);
    expect(screen.queryByText(first.model_display)).not.toBeInTheDocument();
    view.rerender(<TimelineContextValue.Provider value={context}><TurnFooter turnId={same.id} /></TimelineContextValue.Provider>);
    expect(screen.queryByText(first.model_display)).not.toBeInTheDocument();
    view.rerender(<TimelineContextValue.Provider value={context}><TurnFooter turnId={changed.id} /></TimelineContextValue.Provider>);
    expect(screen.getByText(new RegExp(changed.model_display))).toBeVisible();
  });

  it('names an effort change in the catalogue’s words without repeating the unchanged model', () => {
    const first = { ...stored(), effort: 'low' };
    const changed = { ...first, id: 'turn-2', ordinal: 2, effort: 'high' };
    threadWriter.reset(THREAD, { turns: [first, changed] }, false);
    show(<TimelineContextValue.Provider value={context}><TurnFooter turnId={changed.id} /></TimelineContextValue.Provider>);
    expect(screen.queryByText(first.model_display)).not.toBeInTheDocument();
    expect(screen.getByText(i18n.t('timeline.footer.effort', { effort: effortDisplayName('high') }))).toBeVisible();
  });

  it('opens all model inputs of a turn through one footer entry, including a repeated marker', async () => {
    const items = [pointer('context-1', 'record-1'), pointer('context-repeat', 'record-1'), pointer('context-2', 'record-2')]
      .map((item, index) => ({ ...item, thread_id: THREAD, seq_created: index + 1, seq_updated: index + 1 }));
    threadWriter.reset(THREAD, { items, turns: [stored()] }, false);
    show(<TimelineContextValue.Provider value={context}><TurnFooter turnId="turn-1" /></TimelineContextValue.Provider>);
    const footer = document.querySelector('[data-footer="turn-1"]');
    if (!(footer instanceof HTMLElement)) throw new Error('The turn has its footer');
    const links = within(footer).getAllByRole('button', { name: i18n.t('threadViews.given.context') });
    expect(links).toHaveLength(1);
    await userEvent.click(links[0]!);
    expect(given).toHaveBeenCalledExactlyOnceWith(THREAD, { turn: 'turn-1' });
  });

  it('keeps the exact model-input target when the turn has one recorded input', async () => {
    threadWriter.reset(THREAD, { items: [{ ...pointer('context-only', 'record-exact'), seq_created: 1, seq_updated: 1 }], turns: [stored()] }, false);
    show(<TimelineContextValue.Provider value={context}><TurnFooter turnId="turn-1" /></TimelineContextValue.Provider>);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('threadViews.given.context') }));
    expect(given).toHaveBeenCalledExactlyOnceWith(THREAD, { record: 'record-exact' });
  });

  it('keeps a context recorded before the first turn to one compact link to that record', async () => {
    const item = { ...pointer('session-context', 'session-record'), turn_id: null };
    if (item.kind !== 'marker') throw new Error('The context is a history marker');
    show(<TimelineContextValue.Provider value={context}><MarkerLine item={item} /></TimelineContextValue.Provider>);
    expect(screen.getAllByRole('button', { name: i18n.t('threadViews.given.context') })).toHaveLength(1);
    expect(screen.queryByText(i18n.t('threadViews.given.received'), { exact: true })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('threadViews.given.context') }));
    expect(given).toHaveBeenCalledExactlyOnceWith(THREAD, { record: 'session-record' });
  });

  it('uses level-two tool detail sections beneath the thread’s heading', () => {
    const item = toolCall('mcp-1', { kind: 'mcp', server: 'Issues', tool: 'lookup', arguments: { query: 'login' }, result: { found: true }, error: null }, { title: 'Look up login', turn: 'turn-1' });
    if (item.kind !== 'tool_call') throw new Error('The fixture is a tool call');
    show(<><h1>Login</h1><RawCall item={item} /><McpDisplay item={item} /></>);
    const sections = screen.getAllByRole('heading').filter((heading) => heading.tagName !== 'H1');
    expect(sections).toHaveLength(4);
    for (const section of sections) expect(section.tagName).toBe('H2');
    expect(screen.getAllByRole('heading', { name: i18n.t('timeline.mcp.arguments'), level: 2 }).length).toBeGreaterThan(0);
  });

  it.each([null, 'original_author_unavailable', 'unrecognized-private-error'])('keeps the resource resume actor and factual outcome visible for %s', (reason) => {
    const by = { id: 'ada', kind: 'human' as const, display_name: 'Ada Lovelace', username: 'ada', avatar: { text: 'A', color: 1 } };
    const item = marker('resource-enabled', { marker: 'resource_resumed', data: {
      object: { kind: 'provider', id: 'provider-kept' }, name: 'Research API', model: null, by,
      turn_id: 'stopped-turn', reference: 'enable-operation', input_id: reason === null ? 'visible-continue' : null, reason,
    } }, { turn: 'stopped-turn' });
    if (item.kind !== 'marker') throw new Error('The fixture is a history marker');
    show(<TimelineContextValue.Provider value={context}><MarkerLine item={item} /></TimelineContextValue.Provider>);
    expect(screen.getByText('Research API')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Ada Lovelace' })).toBeVisible();
    expect(screen.getByText(new RegExp(i18n.t(reason === null ? 'storage.resumed.continuedTask' : 'storage.resumed.stoppedTask')))).toBeVisible();
    if (reason === 'original_author_unavailable') expect(screen.getByText(new RegExp(i18n.t('storage.resumed.reasons.original_author_unavailable')))).toBeVisible();
    expect(screen.queryByText('unrecognized-private-error')).not.toBeInTheDocument();
    expect(screen.queryByText('visible-continue')).not.toBeInTheDocument();
    expect(screen.queryByText('enable-operation')).not.toBeInTheDocument();
    expect(screen.queryByText('stopped-turn')).not.toBeInTheDocument();
  });

});
