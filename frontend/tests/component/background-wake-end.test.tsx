import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { endBackground, fetchBackground } from '@/api/background';
import { RunningDialog } from '@/features/running/Running';
import type { BackgroundCommand, BackgroundWork, Job, ThreadBackground } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { backgroundRoster, backgroundWake, backgroundWork } from '../doubles/background-wake';
import { show } from './support';

vi.mock('@/api/background', () => ({ fetchBackground: vi.fn(), endBackground: vi.fn() }));
vi.mock('@/api/forks', () => ({ jobQuery: (id: string) => ({ queryKey: ['job', id], queryFn: () => Promise.reject(new Error('No job was requested')) }), isOver: () => false }));
vi.mock('@/realtime/live', () => ({ socket: { status: { getState: () => ({ phase: 'ready' }), subscribe: () => () => undefined } } }));

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
afterEach(() => {
  cleanup();
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
