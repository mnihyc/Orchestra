/** Schedule reads retain native endpoints/cursors and reject late readers; Cancel writes only the schedule id. */
import type { HttpTransport } from '../doubles/http-wire';
import { QueryClient } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { cancelInputSchedule, inputScheduleRunsQuery, topicSchedulesQuery } from '@/api/inputSchedules';
import { inputSchedule, scheduleRun, SCHEDULE, SCHEDULE_TOPIC } from '../doubles/input-schedules';
const wire = vi.hoisted(() => ({ get: vi.fn<HttpTransport>(), remove: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (load) => ({ ...await load<typeof Client>(), api: { GET: wire.get, DELETE: wire.remove } }));
const session = client.session.getState();
beforeEach(() => { vi.clearAllMocks(); client.session.setState({ readerVersion: 901 }); });
afterEach(() => client.session.setState(session));

it('reads the existing complete topic member and paged native run list, with no timer or extra tool operation', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  wire.get.mockResolvedValueOnce({ data: { input_schedules: [inputSchedule()] }, response: new Response() })
    .mockResolvedValueOnce({ data: { items: [scheduleRun()], next_cursor: 'opaque-next' }, response: new Response() });
  expect(await cache.query(topicSchedulesQuery(SCHEDULE_TOPIC))).toEqual([inputSchedule()]);
  expect(wire.get.mock.calls[0]?.[0]).toBe('/topics/{id}/sync');
  expect(await cache.infiniteQuery(inputScheduleRunsQuery(SCHEDULE))).toMatchObject({ pages: [{ items: [scheduleRun()], next_cursor: 'opaque-next' }] });
  expect(wire.get.mock.calls[1]?.[1]).toMatchObject({ params: { path: { id: SCHEDULE }, query: { limit: 100 } } });
  cache.clear();
});
it('rejects a topic result that arrived after the reader changed', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  let finish: ((value: { data: { input_schedules: ReturnType<typeof inputSchedule>[] }; response: Response }) => void) | undefined;
  wire.get.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  const pending = cache.query(topicSchedulesQuery(SCHEDULE_TOPIC));
  const settle = finish; if (settle === undefined) throw new Error('Read did not start');
  client.session.setState({ readerVersion: 902 });
  const result = expect(pending).rejects.toMatchObject({ name: 'AbortError' });
  settle({ data: { input_schedules: [inputSchedule()] }, response: new Response() });
  await result; expect(cache.getQueryData(topicSchedulesQuery(SCHEDULE_TOPIC).queryKey)).toBeUndefined(); cache.clear();
});
it('uses the existing id-only cancellation without cancelling a queued input or reconstructing an author', async () => {
  wire.remove.mockResolvedValue({ response: new Response(null, { status: 204 }) });
  await cancelInputSchedule(SCHEDULE);
  expect(wire.remove).toHaveBeenCalledExactlyOnceWith('/input-schedules/{id}', { params: { path: { id: SCHEDULE } } });
});
