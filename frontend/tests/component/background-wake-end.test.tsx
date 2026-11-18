import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { endBackground, fetchBackground } from '@/api/background';
import { RunningDialog, RunningWorkPanel } from '@/features/running/Running';
import type { BackgroundCommand, BackgroundWork, Job, ThreadBackground } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { backgroundRoster, backgroundWake, backgroundWork } from '../doubles/background-wake';
import { show } from './support';

vi.mock('@/api/background', () => ({ fetchBackground: vi.fn(), endBackground: vi.fn() }));
vi.mock('@/api/forks', () => ({ jobQuery: (id: string) => ({ queryKey: ['job', id], queryFn: () => Promise.reject(new Error('No job was requested')) }), isOver: () => false }));
vi.mock('@/realtime/live', async (loadOriginal) => ({ ...await loadOriginal<object>(), socket: { status: { getState: () => ({ phase: 'ready' }), subscribe: () => () => undefined } } }));

const command: BackgroundCommand = { id: 'reviewed-native-end', segment_id: 'origin-segment', turn_id: null, item_id: null, command: 'Build web',
  command_truncated: false, cwd: null, cwd_redacted: false, place: 'environment', since: null, cpu_percent: null, memory_bytes: null };
const original = backgroundWork();
const originalMatchMedia = Object.getOwnPropertyDescriptor(window, 'matchMedia');
const media = (phone = false) => vi.fn((query: string) => ({ matches: phone && query === '(max-width: 767px)', media: query, onchange: null,
  addListener: vi.fn(), removeListener: vi.fn(), addEventListener: vi.fn(), removeEventListener: vi.fn(), dispatchEvent: () => true }));
const review = (): ThreadBackground => backgroundRoster({ total: 1, list_ref: 'fresh-caller-review', items: [command], wake: backgroundWake({ tasks: [{ ...original, end_id: command.id }] }) });

beforeEach(() => {
  window.matchMedia = media();
  vi.clearAllMocks(); vi.mocked(fetchBackground).mockReset();
  vi.mocked(fetchBackground).mockResolvedValue(review());
  vi.mocked(endBackground).mockImplementation(() => new Promise<Job>(() => undefined));
});
afterEach(async () => {
  cleanup();
  await setLocale('en');
  if (originalMatchMedia === undefined) Reflect.deleteProperty(window, 'matchMedia');
  else Object.defineProperty(window, 'matchMedia', originalMatchMedia);
});

it.each([false, true])('confirms an exact task through its new reviewed end id (phone: %s)', async (phone) => {
  window.matchMedia = media(phone);
  show(<RunningDialog thread="thread-background" close={vi.fn()} endTarget={{ task: original }} />);
  const confirmation = await screen.findByRole('alertdialog', { name: i18n.t('running.confirmTitle') });
  expect(fetchBackground).toHaveBeenCalledTimes(1);
  expect(endBackground).not.toHaveBeenCalled();
  await userEvent.click(within(confirmation).getByRole('button', { name: i18n.t('running.confirm', { count: 1 }) }));
  expect(endBackground).toHaveBeenCalledExactlyOnceWith('thread-background', { list_ref: 'fresh-caller-review', command_ids: [command.id] }, expect.any(String));
  expect(vi.mocked(endBackground).mock.calls[0]?.[1]).not.toHaveProperty('content');
});

it.each(['changed item', 'lost work', 'no End identity', 'no current Interact'] as const)('does not redirect a task after %s', async (change) => {
  const row = review();
  const task: BackgroundWork = { ...original, end_id: command.id };
  if (change === 'changed item') task.item_id = 'later-item';
  if (change === 'lost work') task.state = 'lost';
  if (change === 'no End identity') task.end_id = null;
  if (change === 'no current Interact') task.can_end = false;
  row.wake = backgroundWake({ tasks: [task] });
  vi.mocked(fetchBackground).mockResolvedValue(row);
  show(<RunningDialog thread="thread-background" close={vi.fn()} endTarget={{ task: original }} />);
  await waitFor(() => expect(screen.getByRole('alert')).toBeVisible());
  expect(screen.queryByRole('alertdialog', { name: i18n.t('running.confirmTitle') })).not.toBeInTheDocument();
  expect(endBackground).not.toHaveBeenCalled();
});

it('waits for all pages before resolving the recorded task to its captured native identity', async () => {
  let release: ((value: ThreadBackground) => void) | undefined;
  const pending = new Promise<ThreadBackground>((resolve) => { release = resolve; });
  vi.mocked(fetchBackground).mockResolvedValueOnce(backgroundRoster({ total: 1, list_ref: 'fresh-caller-review', items: [], next_cursor: 'next', wake: backgroundWake() })).mockReturnValueOnce(pending);
  show(<RunningDialog thread="thread-background" close={vi.fn()} endTarget={{ task: original }} />);
  await waitFor(() => expect(fetchBackground).toHaveBeenCalledTimes(2));
  expect(screen.queryByRole('alertdialog', { name: i18n.t('running.confirmTitle') })).not.toBeInTheDocument();
  if (release === undefined) throw new Error('The native second page must be pending');
  release(review());
  expect(await screen.findByRole('alertdialog', { name: i18n.t('running.confirmTitle') })).toBeVisible();
  expect(endBackground).not.toHaveBeenCalled();
});


it.each(['en', 'zh-CN'] as const)('uses the same captured review and explicit End confirmation in the workbench panel in %s', async (locale) => {
  await setLocale(locale);
  show(<RunningWorkPanel thread="thread-background" />);
  const panel = screen.getByRole('region', { name: i18n.t('running.title') });
  await within(panel).findByText('Build web');
  expect(screen.queryByRole('dialog', { name: i18n.t('running.title') })).toBeNull();
  expect(screen.queryByRole('button', { name: i18n.t('running.openPanel') })).toBeNull();
  expect(fetchBackground).toHaveBeenCalledTimes(1);
  await userEvent.click(within(panel).getByRole('button', { name: i18n.t('running.end') }));
  expect(endBackground).not.toHaveBeenCalled();
  const confirmation = await screen.findByRole('alertdialog', { name: i18n.t('running.confirmTitle') });
  expect(within(confirmation).getByText(i18n.t('running.confirmDetail', { count: 1 }))).toBeVisible();
  expect(confirmation).toHaveTextContent(locale === 'en'
    ? 'Orch sends no model prompt; the harness may tell the model the work ended.'
    : 'Orch 不会向模型发送提示；运行程序仍可能通知模型任务已结束。');
  await userEvent.click(within(confirmation).getByRole('button', { name: i18n.t('running.confirm', { count: 1 }) }));
  expect(endBackground).toHaveBeenCalledWith('thread-background', { list_ref: 'fresh-caller-review', command_ids: [command.id] }, expect.any(String));
});

for (const locale of ['en', 'zh-CN'] as const) for (const reason of ['interact_required', 'unsupported', 'no_current_target', 'offline', 'unavailable'] as const) {
  it(`keeps End controls with the native ${reason} reason in ${locale}`, async () => {
    await setLocale(locale);
    const row = review();
    row.can_end = false;
    row.end_reason = reason;
    if (reason === 'unsupported') row.supported = false;
    if (reason === 'offline' || reason === 'unavailable') row.state = reason;
    vi.mocked(fetchBackground).mockResolvedValue(row);
    show(<RunningWorkPanel thread="thread-background" />);
    await screen.findByText('Build web');
    const explanation = i18n.t(`running.endReasons.${reason}`);
    const end = screen.getByRole('button', { name: i18n.t('running.end') });
    const selected = screen.getByRole('button', { name: i18n.t('running.endSelected', { count: 0 }) });
    const all = screen.getByRole('button', { name: i18n.t('running.endAll', { count: 1 }) });
    for (const control of [end, selected, all]) {
      expect(control).toHaveAttribute('aria-disabled', 'true');
      expect(control).toHaveAttribute('aria-description', explanation);
      await userEvent.click(control);
    }
    await userEvent.hover(end);
    expect(await screen.findByRole('tooltip')).toHaveTextContent(explanation);
    expect(screen.queryByRole('alertdialog')).toBeNull();
    expect(screen.queryByRole('checkbox')).toBeNull();
    expect(endBackground).not.toHaveBeenCalled();
  });
}

it('keeps a stale command disabled beside another currently endable target', async () => {
  const row = review();
  row.items = [{ ...command, end_reason: 'no_current_target' }, { ...command, id: 'live-command', command: 'Build docs' }];
  row.total = 2;
  row.wake = backgroundWake({ tasks: [] });
  vi.mocked(fetchBackground).mockResolvedValue(row);
  show(<RunningWorkPanel thread="thread-background" />);
  await screen.findByText('Build docs');
  const rows = screen.getAllByRole('listitem');
  const stale = rows.find((row) => within(row).queryByText('Build web') !== null);
  const current = rows.find((row) => within(row).queryByText('Build docs') !== null);
  if (stale === undefined || current === undefined) throw new Error('Both native commands must remain visible');
  expect(within(stale).getByRole('button', { name: i18n.t('running.end') })).toHaveAttribute('aria-disabled', 'true');
  expect(within(current).getByRole('button', { name: i18n.t('running.end') })).not.toHaveAttribute('aria-disabled');
  expect(screen.getByRole('button', { name: i18n.t('running.endAll', { count: 2 }) })).toHaveAttribute('aria-description', i18n.t('running.endReasons.no_current_target'));
  await userEvent.click(within(current).getByRole('button', { name: i18n.t('running.end') }));
  const confirmation = await screen.findByRole('alertdialog', { name: i18n.t('running.confirmTitle') });
  await userEvent.click(within(confirmation).getByRole('button', { name: i18n.t('running.confirm', { count: 1 }) }));
  expect(endBackground).toHaveBeenCalledWith('thread-background', { list_ref: 'fresh-caller-review', command_ids: ['live-command'] }, expect.any(String));
});

it('keeps checking distinct from a failed read and refreshes without lending End', async () => {
  vi.mocked(fetchBackground).mockReturnValueOnce(new Promise<ThreadBackground>(() => undefined));
  show(<RunningWorkPanel thread="thread-background" />);
  const all = screen.getByRole('button', { name: i18n.t('running.endAllUnknown') });
  expect(all).toHaveAttribute('aria-description', i18n.t('running.endChecking'));
  expect(all).toHaveAttribute('aria-disabled', 'true');
  expect(endBackground).not.toHaveBeenCalled();
  await waitFor(() => expect(fetchBackground).toHaveBeenCalledOnce());
  cleanup();
  vi.mocked(fetchBackground).mockRejectedValueOnce(new Error('Current roster read failed')).mockResolvedValueOnce(review());
  show(<RunningWorkPanel thread="thread-background" />);
  await screen.findByRole('alert');
  expect(screen.getByRole('button', { name: i18n.t('running.endAllUnknown') })).toHaveAttribute('aria-description', i18n.t('running.endReadFailed'));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('running.refresh') }));
  await screen.findByText('Build web');
  expect(screen.getByRole('button', { name: i18n.t('running.end') })).not.toHaveAttribute('aria-disabled');
  expect(endBackground).not.toHaveBeenCalled();
});

it('does not turn an offline background observation into an empty live set', async () => {
  vi.mocked(fetchBackground).mockResolvedValue(backgroundRoster({ state: 'offline', total: null, items: [], can_end: false }));
  show(<RunningWorkPanel thread="thread-background" />);
  expect(await screen.findByText(i18n.t('running.offline'))).toBeVisible();
  expect(screen.queryByText(i18n.t('running.empty'))).toBeNull();
  expect(endBackground).not.toHaveBeenCalled();
});
