/** Native history removal and reconstruction stay factual, localized and free of initial card reads. */
import { fireEvent, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MarkerLine, markerSentence } from '@/features/timeline/items/Marker';
import { buildRows } from '@/features/timeline/rows';
import { EMPTY_VIEW, viewFromRows } from '@/realtime/stores/thread';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import { waitingWords } from '@/features/timeline/waiting';
import { SettingRow } from '@/features/settings/SettingRow';
import { withGroup } from '@/features/settings/settingsModel';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { formatBytes } from '@/lib/format';
import type { MarkerAgentHistory, Thread, UserRef } from '@/generated/api-types';
import { marker } from '../doubles/thread-rows';
import { show } from './support';

const openGuide = vi.hoisted(() => vi.fn());
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => openGuide }));

const context: TimelineContext = { threadId: 'history-thread', threadName: 'History', detail: 'steps', recent: new Set(), layout: 'one', setLayout: () => undefined,
  meId: null, canInteract: false, canManage: false, live: true, lit: null, selection: NOT_PICKING };
let locale = currentLocale();
let theme: string | null = null;
beforeEach(() => { locale = currentLocale(); theme = document.documentElement.getAttribute('data-theme'); });
afterEach(async () => { await setLocale(locale); if (theme === null) document.documentElement.removeAttribute('data-theme'); else document.documentElement.setAttribute('data-theme', theme); vi.unstubAllGlobals(); openGuide.mockClear(); });
const data = (changes: Partial<MarkerAgentHistory> = {}): MarkerAgentHistory => ({ segment_id: 'source-segment', program: 'Codex', bytes: 4096, removed_at: '2026-10-20T12:00:00Z', by: null, recovered: null, ...changes });
const user = (id: string): UserRef => ({ id, kind: 'human', username: id, display_name: id, avatar: { text: id.slice(0, 1), color: 0 } });
function draw(value: MarkerAgentHistory) {
  const item = marker('history-marker', { marker: 'agent_history', data: value });
  if (item.kind !== 'marker') throw new Error('The fixture is a marker');
  return show(<TimelineContextValue.Provider value={context}><MarkerLine item={item} /></TimelineContextValue.Provider>);
}

it('keeps removal and reconstruction visible when one person does both within a minute', () => {
  const by = user('history-manager');
  const items = [marker('removed', { marker: 'agent_history', data: data({ by }) }), marker('recovered', { marker: 'agent_history', data: data({ by, recovered: 'native' }) })]
    .map((item, index) => ({ ...item, seq_created: index + 1, seq_updated: index + 1 }));
  const view = viewFromRows(EMPTY_VIEW, { items }, false);
  const rows = buildRows(view, { detail: 'steps', hideHistory: false, foldOpened: new Set(), meId: null });
  expect(rows.filter((row) => row.kind === 'markers').map((row) => row.ids)).toEqual([['removed'], ['recovered']]);
});

it('keeps the actor in a copied history sentence when its visual row uses a person chip', () => {
  const by = user('history-manager');
  const text = markerSentence({ marker: 'agent_history', data: data({ by }) }, i18n.t.bind(i18n), { locale: 'en' });
  expect(text).toContain(by.display_name);
  expect(text).toContain('Codex');
});

it('opens the agent history guide from the actual registry editor row without an API read', () => {
  const def = SETTINGS_REGISTRY.find((setting) => setting.key === 'retention.agent_history_idle_days');
  if (def === undefined) throw new Error('The history policy is shipped');
  const fetch = vi.fn(); vi.stubGlobal('fetch', fetch);
  show(<ul><SettingRow def={withGroup(def)} setting={{ key: def.key, value: 7, own: null,
    source: { scope: 'default', node: null }, locked_by: null, writable: true, parts: [] }}
    target={{ kind: 'system' }} place={{ name: 'System', owner: null, access: { permissions: ['settings.manage'] }, kind: null }} /></ul>);
  const hint = screen.getByRole('button', { name: i18n.t('help.hint.label', { term: def.title }) });
  expect(hint).toHaveAttribute('data-help-article', 'settings-and-usage');
  expect(hint).toHaveAttribute('data-help-heading', 'agent-history');
  fireEvent.click(hint);
  fireEvent.click(screen.getByRole('button', { name: i18n.t('helpRelease.openEntry') }));
  expect(openGuide).toHaveBeenCalledExactlyOnceWith('settings-and-usage', 'agent-history');
  expect(fetch).not.toHaveBeenCalled();
});

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('agent history in %s / %s', (language, look) => {
  beforeEach(async () => { await setLocale(language); document.documentElement.setAttribute('data-theme', look); });
  it('names the program, measured bytes, automatic actor and actual time without a read', () => {
    const fetch = vi.fn(); vi.stubGlobal('fetch', fetch);
    draw(data());
    expect(screen.getByText(i18n.t('agentHistory.automatic'))).toBeVisible();
    expect(screen.getByText(`${i18n.t('agentHistory.removed', { program: 'Codex' })} · ${formatBytes(4096, language)}`)).toBeVisible();
    expect(document.querySelector('time')).toHaveAttribute('dateTime');
    expect(fetch).not.toHaveBeenCalled();
  });
  it('uses the shared person chip for an explicit actor and leaves an unknown size unknown', () => {
    const by = user('history-manager');
    draw(data({ by, bytes: null }));
    expect(screen.getByText(by.display_name)).toBeVisible();
    expect(screen.getByText(`${i18n.t('agentHistory.removedBy', { program: 'Codex' })} · –`)).toBeVisible();
    expect(screen.queryByText(i18n.t('agentHistory.automatic'))).not.toBeInTheDocument();
  });
  it.each(['native', 'carry', 'summary', 'brief', 'file', 'none'] as const)('states the actual %s reconstruction outcome', (mode) => {
    const sentence = markerSentence({ marker: 'agent_history', data: data({ recovered: mode }) }, i18n.t.bind(i18n), { locale: language });
    draw(data({ recovered: mode }));
    expect(screen.getByText(sentence)).toBeVisible();
    expect(sentence).toContain(i18n.t(`agentHistory.history.${mode}`));
    expect(document.querySelectorAll('time')).toHaveLength(2);
  });
  it('shows an automatic removal wait without a false error or manager action', () => {
    const thread = { id: 'history-thread', state: 'blocked', lock: null, blocked: { reason: 'error', detail: i18n.getFixedT('en')('agentHistory.removingDetail') }, binding: null, access: { permissions: ['thread.view'] }, owner: user('owner') } as unknown as Thread;
    show(<BlockedBanner thread={thread} onProceed={vi.fn()} />);
    expect(screen.getByText(i18n.t('agentHistory.removing'))).toBeVisible();
    expect(waitingWords(thread, i18n.t.bind(i18n))).toBe(i18n.t('agentHistory.removing'));
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});
