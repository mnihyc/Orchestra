/** A native resume limitation is a localized history line, without a false failure or an action. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MarkerLine, markerSentence } from '@/features/timeline/items/Marker';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { buildRows } from '@/features/timeline/rows';
import { EMPTY_VIEW, viewFromRows } from '@/realtime/stores/thread';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { marker } from '../doubles/thread-rows';
import { show } from './support';

const context: TimelineContext = {
  threadId: 'resumed-thread', threadName: 'Resumed work', detail: 'steps', recent: new Set(),
  layout: 'one', setLayout: () => undefined, meId: null, canInteract: true, canManage: false,
  live: true, lit: null, selection: NOT_PICKING,
};
let locale = currentLocale();
let theme: string | null = null;
beforeEach(() => { locale = currentLocale(); theme = document.documentElement.getAttribute('data-theme'); });
afterEach(async () => {
  await setLocale(locale);
  if (theme === null) document.documentElement.removeAttribute('data-theme');
  else document.documentElement.setAttribute('data-theme', theme);
  vi.unstubAllGlobals();
});

describe.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('resumed instructions in %s / %s', (language, look) => {
  it('states what happens next without suggesting that the completed turn failed', async () => {
    await setLocale(language);
    document.documentElement.setAttribute('data-theme', look);
    const fetch = vi.fn();
    vi.stubGlobal('fetch', fetch);
    const payload = { marker: 'instructions_deferred', data: { program: 'Codex' } } as const;
    const item = { ...marker('instructions-note', payload), seq_created: 1, seq_updated: 1 };
    if (item.kind !== 'marker') throw new Error('The fixture is a history marker');
    const view = viewFromRows(EMPTY_VIEW, { items: [item] }, false);
    const rows = buildRows(view, { detail: 'steps', hideHistory: false, foldOpened: new Set(), meId: null });
    expect(rows.filter((row) => row.kind === 'markers').flatMap((row) => row.ids)).toEqual([item.id]);
    show(<TimelineContextValue.Provider value={context}><MarkerLine item={item} /></TimelineContextValue.Provider>);
    const sentence = markerSentence(payload, i18n.t.bind(i18n), { locale: language });
    expect(sentence).toBe(i18n.t('timeline.marker.instructionsDeferred', { program: 'Codex' }));
    expect(screen.getByText(sentence)).toBeVisible();
    expect(sentence).toContain('Codex');
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    const time = screen.getByRole('button', { name: i18n.t('timeline.exactTime') });
    // The exact moment is available to a keyboard and a touch reader, without expanding the history line.
    expect(document.querySelector('time')).toBeNull();
    expect(screen.getByText(sentence).closest('[title]')).not.toBeNull();
    const user = userEvent.setup();
    await user.tab();
    expect(time).toHaveFocus();
    await user.keyboard('{Enter}');
    let moment = screen.getByRole('dialog', { name: i18n.t('timeline.exactTime') }).querySelector('time');
    expect(moment).toBeVisible();
    expect(moment).toHaveAttribute('datetime', item.created_at);
    expect(moment?.textContent).toBe(time.getAttribute('title'));
    await user.keyboard('{Escape}');
    expect(time).toHaveFocus();
    await user.pointer({ keys: '[TouchA]', target: time });
    moment = screen.getByRole('dialog', { name: i18n.t('timeline.exactTime') }).querySelector('time');
    expect(moment).toBeVisible();
    expect(fetch).not.toHaveBeenCalled();
  });
});
