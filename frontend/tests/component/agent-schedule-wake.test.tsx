/** A rendered ordinary input receives the wake marker only from its exact native agent schedule/run. */
import type { HttpTransport } from '../doubles/http-wire';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import type { InputSchedule } from '@/api/inputSchedules';
import { ScheduleWakeMarker } from '@/features/schedules/WakeMarker';
import { i18n, setLocale } from '@/i18n';
import { inputSchedule, scheduleRun, SCHEDULE, SCHEDULE_THREAD } from '../doubles/input-schedules';
import { show } from './support';
const wire = vi.hoisted(() => ({ get: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (load) => ({ ...await load<typeof Client>(), api: { GET: wire.get } }));
const session = client.session.getState();
const hint = `agent-schedule:${SCHEDULE}:2030-01-01T21:30:00Z`;
const draw = (enabled = true) => show(<ScheduleWakeMarker thread={SCHEDULE_THREAD} input="scheduled-input" turn="scheduled-turn" clientId={hint} enabled={enabled} />);
const answer = (data: unknown) => ({ data, response: new Response() });
beforeEach(() => { wire.get.mockReset(); client.session.setState({ readerVersion: 1001 }); });
afterEach(async () => { cleanup(); client.session.setState(session); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('marks the actual latest run, without reading an unnecessary history page, in %s', async (locale) => {
  await setLocale(locale); wire.get.mockResolvedValue(answer(inputSchedule({ last_run: scheduleRun() }))); draw();
  expect(await screen.findByText(i18n.t('agentSchedules.woken'))).toBeVisible();
  expect(wire.get).toHaveBeenCalledExactlyOnceWith('/input-schedules/{id}', expect.objectContaining({ params: { path: { id: SCHEDULE } } }));
});

it('walks native opaque cursors to prove an older visible input instead of imposing a max-runs cap', async () => {
  wire.get.mockResolvedValueOnce(answer(inputSchedule({ agent: { ...inputSchedule().agent!, runs: 125, max_runs: 200 }, last_run: scheduleRun({ input_id: 'newer-input', turn_id: 'newer-turn' }) })))
    .mockResolvedValueOnce(answer({ items: [scheduleRun({ input_id: 'newer-input', turn_id: 'newer-turn' })], next_cursor: 'native-older' }))
    .mockResolvedValueOnce(answer({ items: [scheduleRun()], next_cursor: 'still-older' }));
  draw(); expect(await screen.findByText(i18n.t('agentSchedules.woken'))).toBeVisible();
  expect(wire.get).toHaveBeenCalledTimes(3);
  expect(wire.get.mock.calls[2]?.[1]).toMatchObject({ params: { path: { id: SCHEDULE }, query: { limit: 100, cursor: 'native-older' } } });
});

it.each(['forged', 'foreign', 'people', 'queued'] as const)('does not turn a %s client hint or row into proof', async (kind) => {
  const row = kind === 'people' ? inputSchedule({ kind: undefined, agent: undefined })
    : kind === 'foreign' ? inputSchedule({ agent: { ...inputSchedule().agent!, thread_id: 'foreign-thread' } }) : inputSchedule();
  const run = kind === 'queued' ? scheduleRun({ state: 'queued', turn_id: null, started_at: null }) : scheduleRun({ input_id: 'other-input' });
  wire.get.mockResolvedValueOnce(answer(row)).mockResolvedValueOnce(answer({ items: [run], next_cursor: null }));
  draw(); const count = kind === 'foreign' || kind === 'people' ? 1 : 2;
  await waitFor(() => expect(wire.get).toHaveBeenCalledTimes(count));
  expect(screen.queryByText(i18n.t('agentSchedules.woken'))).toBeNull();
});

it('starts no hidden, stored or imported-session lookup without a real ordinary input and turn', () => {
  draw(false);
  show(<ScheduleWakeMarker thread={SCHEDULE_THREAD} input={null} turn={null} clientId={hint} enabled />);
  show(<ScheduleWakeMarker thread={SCHEDULE_THREAD} input="input" turn="turn" clientId={null} enabled />);
  expect(wire.get).not.toHaveBeenCalled();
});

it('does not publish the old reader proof or walk another page after the reader changes', async () => {
  let finish: ((value: { data: InputSchedule; response: Response }) => void) | undefined;
  wire.get.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }))
    .mockResolvedValueOnce(answer(inputSchedule({ kind: undefined, agent: undefined })));
  draw(); await waitFor(() => expect(wire.get).toHaveBeenCalledOnce());
  const settle = finish; if (settle === undefined) throw new Error('Schedule metadata read did not start');
  act(() => client.session.setState({ readerVersion: 1002 }));
  await waitFor(() => expect(wire.get).toHaveBeenCalledTimes(2));
  await act(() => { settle({ data: inputSchedule({ last_run: scheduleRun() }), response: new Response() }); return Promise.resolve(); });
  expect(screen.queryByText(i18n.t('agentSchedules.woken'))).toBeNull(); expect(wire.get).toHaveBeenCalledTimes(2);
});
