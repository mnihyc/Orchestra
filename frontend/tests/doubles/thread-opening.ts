/** The first thread screen reuses the fake world's native readers without making nested HTTP requests. */
import type { Problem, ThreadOpening, TopicSync } from '../../src/generated/api-types.ts';
import { isAgentSchedule } from '../../src/features/schedules/facts.ts';
import { Refusal, type Person, type World } from './world.ts';

export async function threadOpening(world: World, person: Person, threadId: string, limit: number): Promise<ThreadOpening> {
  async function read(path: string, query = new URLSearchParams()): Promise<unknown> {
    const search = query.toString();
    return (await world.requestFeeds.read(person, search === '' ? path : `${path}?${search}`)).body;
  }
  async function optional(path: string): Promise<unknown> {
    try { return await read(path); }
    catch (error) { if (error instanceof Refusal && error.answer.status === 403) return null; throw error; }
  }
  // Sync checks the thread's native admission before any of its related projections are read.
  const sync = await read(`/threads/${threadId}/sync`, new URLSearchParams({ limit: String(limit) })) as ThreadOpening['sync'];
  // An aside has its own conversation and draft, with the choices and limits of its parent.
  const parent = sync.thread.side_of?.id;
  const choices = parent ?? threadId;
  let pending: ThreadOpening['pending_agent_schedules'];
  if (sync.projection?.follows_active !== false) {
    try {
      const topic = await read(`/topics/${sync.thread.topic_id}/sync`) as TopicSync;
      pending = { status: 'ready', data: { seq: topic.seq, items: topic.input_schedules.flatMap((row) =>
        isAgentSchedule(row, threadId) && row.state === 'active' && row.next_run_at !== null
          ? [{ id: row.id, name: row.name, state: row.state, next_run_at: row.next_run_at }] : []) } };
    } catch (error) {
      if (!(error instanceof Refusal) || error.answer.status !== 403) throw error;
      pending = { status: 'failed', problem: error.answer.body as Problem, retry_after: null };
    }
  }
  const opening: ThreadOpening = {
    sync,
    draft: await optional(`/threads/${threadId}/draft`) as ThreadOpening['draft'],
    parameters: structuredClone(await optional(`/threads/${choices}/parameters`)) as ThreadOpening['parameters'],
    limits: await optional(`/threads/${choices}/limits`) as ThreadOpening['limits'],
    gateway: sync.projection?.follows_active === false ? null : {
      status: 'ready' as const, data: await read(`/threads/${threadId}/gateway`) as NonNullable<Extract<NonNullable<ThreadOpening['gateway']>, { status: 'ready' }>['data']>,
    },
    ...(pending === undefined ? {} : { pending_agent_schedules: pending }),
  };
  if (opening.parameters !== null) {
    opening.parameters.model.options = opening.parameters.model.options.filter((option) => option.model_name === opening.parameters?.model.value);
    for (const option of opening.parameters.model.options) {
      option.option_constraints = {};
      delete option.parameters.request_options;
      delete option.parameters.offered;
    }
    opening.parameters.model.options_partial = true;
  }
  return opening;
}
