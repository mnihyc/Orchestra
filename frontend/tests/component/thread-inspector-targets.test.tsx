/** Conversation inspectors do not treat a file's permission supplier as their conversation target. */
import { createRef } from 'react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { cleanup, screen, waitFor } from '@testing-library/react';
import type * as Threads from '@/api/threads';
import type * as Live from '@/realtime/live';
import type * as Router from '@tanstack/react-router';
import type { ThreadOutlinePage } from '@/generated/api-types';
import type { RawCapture, RawRetention } from '@/generated/api-types';
import { CaptureFacts } from '@/features/inspector/CaptureFacts';
import { OutlineView } from '@/features/inspector/Outline';
import { ParametersView } from '@/features/inspector/Parameters';
import { ViewInstanceProvider } from '@/features/workbench/instance';
import { ThreadReadProvider } from '@/realtime/threadReadContext';
import { resultThreadRead } from '@/realtime/threadScope';
import { threadWriter } from '@/realtime/stores/thread';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { snapshot } from '../unit/continuation-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ outline: vi.fn<typeof Threads.outlineQuery>(), parameters: vi.fn<typeof Threads.parametersQuery>(), follows: vi.fn() }));
const appearance = useAppearance.getState();
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), useParams: () => ({ threadId: 'unrelated-route-thread' }) }));
vi.mock('@/api/threads', async (load) => {
  const actual = await load<typeof Threads>();
  calls.outline.mockImplementation((read) => ({ ...actual.outlineQuery(read), queryFn: () => new Promise<ThreadOutlinePage>(() => undefined) }));
  calls.parameters.mockImplementation(actual.parametersQuery);
  return { ...actual, outlineQuery: calls.outline, parametersQuery: calls.parameters };
});
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useThreadFeed: (target: unknown) => { calls.follows(target); } }));
afterEach(async () => { cleanup(); threadWriter.clear(); calls.outline.mockClear(); calls.parameters.mockClear(); calls.follows.mockClear(); useAppearance.setState(appearance); await setLocale('en'); });

it('file/workspace focus shows the existing empty state and opens no conversation or parameter reader', () => {
  show(<ViewInstanceProvider value={{ id: 'following-panel', root: createRef(), target: { kind: 'environment', id: 'workspace-a', workspace: { environmentId: 'workspace-a', threadId: 'permission-supplier' } }, headingOwner: 'file-view', content: false, focusedContent: false }}>
    <ThreadReadProvider read={null}><OutlineView tab={{ id: 'outline', view: 'outline' }} area="right" /><ParametersView tab={{ id: 'parameters', view: 'parameters' }} area="right" /></ThreadReadProvider>
  </ViewInstanceProvider>);
  expect(screen.getAllByText(i18n.t('inspector.frame.noThread'))).toHaveLength(2);
  expect(calls.outline).not.toHaveBeenCalled(); expect(calls.parameters).not.toHaveBeenCalled();
  expect(calls.follows.mock.calls.every(([target]) => target === null)).toBe(true);
});

it('an actual thread can read its outline without an environment, using its own history owner', async () => {
  const id = 'actual-thread', read = resultThreadRead(id, 'actual-outline'), cut = snapshot();
  threadWriter.reset(read, { ...cut, thread: { ...cut.thread, id, environment: null } }, false);
  show(<ViewInstanceProvider value={{ id: 'actual-outline', root: createRef(), target: { kind: 'thread', id }, headingOwner: 'actual-view', content: false, focusedContent: false }}>
    <ThreadReadProvider read={read}><OutlineView tab={{ id: 'actual-outline', view: 'outline' }} area="right" /></ThreadReadProvider>
  </ViewInstanceProvider>);
  await waitFor(() => expect(calls.outline).toHaveBeenCalledWith(read, { q: '' }));
  expect(screen.queryByText(i18n.t('inspector.frame.noThread'))).not.toBeInTheDocument();
  expect(calls.follows.mock.calls.every(([target]) => target === id)).toBe(true);
});

const captured = (over: Partial<RawCapture> = {}): RawCapture => ({
  observed_range: { from: 3, through: 12, count: 4 }, retained_ranges: [{ from: 9, through: 10, count: 2 }],
  omission_counts_by_cause: [{ cause: 'capture_off', count: 0 }, { cause: 'masking_unavailable', count: 1 },
    { cause: 'capture_allowance', count: 1 }, { cause: 'retention_elapsed', count: 0 }],
  oversize_notes: 1, truncated_notes: 1, earlier_unreported: true,
  limits: { native_n: 12, captured_at: '2026-10-20T00:00:00Z', frame_bytes: 64, head_bytes: 8, frames_per_segment: 2,
    frame_source: { scope: 'default', node: null }, head_source: { scope: 'default', node: null }, frames_source: { scope: 'system', node: null } },
  captured_at: '2026-10-20T00:00:00Z', ...over,
});
const retention = (days: number): RawRetention => ({ days, source: { scope: 'system', node: null }, measured_at: '2026-10-21T00:00:00Z' });

it.each(['en', 'zh-CN'] as const)('keeps absent capture provenance unreported when old frames have no receipt in %s', async (locale) => {
  await setLocale(locale); useAppearance.setState({ locale });
  const user = userEvent.setup();
  show(<CaptureFacts />);
  expect(screen.queryByText(locale === 'en' ? 'Capture incomplete' : '捕获内容不完整')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  expect(screen.getAllByText(i18n.t('factDisclosure.notReported')).length).toBeGreaterThan(0);
  expect(document.querySelector('[data-fact-disclosure-body]')).not.toHaveTextContent(/20,000|65536|65,536/);
  expect(calls.outline).not.toHaveBeenCalled(); expect(calls.parameters).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('preserves recorded capture limits when the current retention policy changes in %s', async (locale) => {
  await setLocale(locale); useAppearance.setState({ locale });
  const user = userEvent.setup(), record = captured();
  const view = show(<CaptureFacts capture={record} retention={retention(14)} />);
  expect(screen.getByText(locale === 'en' ? 'Capture incomplete' : '捕获内容不完整')).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('common.details') }));
  const body = document.querySelector('[data-fact-disclosure-body]');
  expect(body).toHaveTextContent(formatBytes(64, locale));
  expect(body).toHaveTextContent(locale === 'en' ? 'Masking inputs unavailable' : '脱敏依据不可用');
  expect(body?.querySelector(`time[datetime="${record.limits.captured_at}"]`)).not.toBeNull();
  view.rerender(<CaptureFacts capture={record} retention={retention(7)} />);
  expect(document.querySelector('[data-fact-group="capture"]')).toHaveTextContent(formatBytes(64, locale));
  expect(document.querySelector('[data-fact-group="retention"]')).not.toHaveTextContent(formatBytes(64, locale));
  view.rerender(<CaptureFacts />);
  expect(screen.queryByText(locale === 'en' ? 'Capture incomplete' : '捕获内容不完整')).not.toBeInTheDocument();
  expect(document.querySelector('[data-fact-disclosure-body]')).not.toHaveTextContent(locale === 'en' ? 'Masking inputs unavailable' : '脱敏依据不可用');
});

it.each(['en', 'zh-CN'] as const)('does not turn a size note or native-number gaps into an invented capture omission in %s', async (locale) => {
  await setLocale(locale); useAppearance.setState({ locale });
  show(<CaptureFacts capture={captured({ earlier_unreported: false, truncated_notes: 0,
    omission_counts_by_cause: [], retained_ranges: [{ from: 3, through: 30000, count: 2 }],
  })} retention={retention(14)} />);
  expect(screen.queryByText(locale === 'en' ? 'Capture incomplete' : '捕获内容不完整')).not.toBeInTheDocument();
  expect(screen.getByText(/3.*30.?000/)).toBeVisible();
});
