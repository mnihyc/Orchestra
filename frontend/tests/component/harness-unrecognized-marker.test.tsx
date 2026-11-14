/** The shared passive observation keeps one native marker vocabulary for every program. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import { useEffect } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Personal from '@/api/personal';
import { client } from '@/api/client';
import { StoredTimeline } from '@/features/timeline/Timeline';
import { useTimelinePreferences } from '@/features/timeline/preferences';
import type { ThreadSync } from '@/generated/api-types';
import { markerSentence } from '@/features/timeline/items/Marker';
import type { MarkerPayload } from '@/features/timeline/markerTypes';
import { i18n, setLocale } from '@/i18n';
import { formatNumber } from '@/lib/format';
import { agentMessage, command, marker, turn, userMessage } from '../doubles/thread-rows';
import { A, snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const reader = vi.hoisted(() => ({ state: {} }));
vi.mock('@/api/personal', async (original) => ({ ...await original<typeof Personal>(),
  meStateQuery: { queryKey: ['me-state'], staleTime: Infinity, queryFn: () => Promise.resolve(reader.state) },
  setMeState: () => Promise.resolve(),
}));
const session = client.session.getState();
beforeEach(() => { reader.state = {}; client.session.setState({ status: 'signed_in', userId: 'notice-reader', readerVersion: 1 }); });
afterEach(() => { cleanup(); window.localStorage.removeItem('orch.timeline.hideHistory'); client.session.setState(session); });

function HiddenHistory({ snapshot }: { snapshot: ThreadSync }) {
  const preferences = useTimelinePreferences(snapshot.thread.id);
  useEffect(() => {
    preferences.setHideHistory(true);
    return () => preferences.setHideHistory(false);
  }, [preferences.setHideHistory]);
  return <div data-notice-detail={preferences.detail}><StoredTimeline snapshot={snapshot} /></div>;
}

for (const locale of ['en', 'zh-CN'] as const) for (const mode of ['normal', 'verbose', 'diagnostics'] as const)
  it(`keeps native warnings and unreadable-diff notices outside folded work in ${locale}/${mode}`, async () => {
    await setLocale(locale);
    const source = snapshot(), done = turn('notice-turn', { ordinal: 1, state: 'completed', inputId: 'notice-input' });
    reader.state = { timeline: { threads: { [source.thread.id]: { work_detail: mode } } } };
    const message = 'The selected model changed. <script>plain text</script>';
    const report = (id: string, change: string, value: unknown) => marker(id, { marker: 'harness_changed', data: {
      program: 'Codex', change, subject: null, previous: { state: 'unsupported' }, expected: { state: 'unsupported' }, value,
      changed: false, diverged: false, next_turn: false, turn_id: done.id, reported_at: '2026-11-14T10:00:00Z',
    } }, { turn: done.id });
    const items = [userMessage('notice-input', 'Check this change', { turn: done.id, author: source.thread.owner }),
      command('notice-work', { turn: done.id, command: 'check files', exit: 0 }),
      report('notice-warning', 'warning', { message }), report('notice-diff', 'turn_diff_unavailable', { reason: 'format_unknown' }),
      agentMessage('notice-answer', 'The answer is ready.', { turn: done.id })].map((item) => ({ ...item, thread_id: source.thread.id, continuation_id: A }));
    const view = show(<HiddenHistory snapshot={{ ...source, turns: [done], items }} />);
    const warning = await screen.findByText(i18n.t('harnessChanges.warning', { program: 'Codex', message }));
    const unreadable = await screen.findByText(i18n.t('harnessChanges.diffUnreadable', { program: 'Codex' }));
    await waitFor(() => {
      expect(view.container.querySelector('[data-notice-detail]')).toHaveAttribute('data-notice-detail', mode);
      expect(warning).toBeVisible(); expect(unreadable).toBeVisible();
    });
    expect(warning.closest('[data-work]')).toBeNull(); expect(unreadable.closest('[data-work]')).toBeNull();
    expect(view.container.querySelector('script')).toBeNull();
  });

afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`explains an unreadable diff in ${locale} without inventing native words`, async () => {
  await setLocale(locale);
  const payload: MarkerPayload = { marker: 'harness_changed', data: {
    program: 'Codex', change: 'turn_diff_unavailable', subject: null, previous: { state: 'unsupported' },
    expected: { state: 'unsupported' }, value: { reason: 'format_unknown' }, changed: false, diverged: false,
    next_turn: false, turn_id: null, reported_at: '2026-11-14T08:00:00Z',
  } };
  const line = markerSentence(payload, i18n.t, { locale });
  expect(line).toContain('Codex');
  expect(line).toContain(locale === 'en' ? 'Earlier changed files were kept' : '保留此前的文件更改');
  expect(line).not.toContain('format_unknown');
});
for (const locale of ['en', 'zh-CN'] as const) it(`shows a native warning's actual words in ${locale}`, async () => {
  await setLocale(locale);
  const message = 'The model changed. <script>plain text</script>';
  const payload: MarkerPayload = { marker: 'harness_changed', data: {
    program: 'Codex', change: 'warning', subject: null, previous: { state: 'unsupported' },
    expected: { state: 'unsupported' }, value: { message }, changed: false, diverged: false,
    next_turn: false, turn_id: null, reported_at: '2026-11-14T08:00:00Z',
  } };
  expect(markerSentence(payload, i18n.t, { locale })).toBe(i18n.t('harnessChanges.warning', { program: 'Codex', message }));
});
for (const locale of ['en', 'zh-CN'] as const) for (const program of ['Codex', 'Claude Code', 'DeepSeek Harness', 'Fourth program']) it(`names a counted unknown ${program} event in ${locale}`, async () => {
  await setLocale(locale);
  for (const count of [1, 1000]) {
    const payload: MarkerPayload = { marker:'harness_changed', data:{
      program,change:'unrecognized_event',subject:'future/update',previous:{state:'unsupported'},
      expected:{state:'unsupported'},value:{count},changed:false,diverged:false,next_turn:false,
      turn_id:null,reported_at:'2026-10-08T14:00:00Z',
    }};
    const sentence=markerSentence(payload,i18n.t,{locale});
    expect(sentence).toBe(i18n.t(count===1?'harnessChanges.unrecognized':'harnessChanges.unrecognizedCount',{
      program,name:'future/update',number:formatNumber(count,locale),
    }));
    expect(sentence).not.toContain('reported');
    expect(sentence).not.toContain('settings.');
  }
});
