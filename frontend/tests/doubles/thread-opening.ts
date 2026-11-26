/** The first thread screen reuses the fake world's native readers without making nested HTTP requests. */
import type { AuthResult, AuthThreadOpening, Problem, ThreadOpening, TopicSync } from '../../src/generated/api-types.ts';
import { isAgentSchedule } from '../../src/features/schedules/facts.ts';
import { Refusal, type Answer, type Person, type World } from './world.ts';

export async function sessionThreadOpening(world: World, threadId: string, cookies: Record<string, string>, csrf: string | undefined, requestId: string): Promise<Answer> {
  const restored = world.restoreSession(cookies, csrf);
  if (restored.body === null) return restored;
  const session = restored.body as AuthResult;
  const opening = await threadOpeningPart(world, world.browserPerson(`Bearer ${session.access_token}`), threadId, requestId);
  return { ...restored, body: { session, opening } satisfies AuthThreadOpening };
}

export async function threadOpeningPart(world: World, reader: Person, threadId: string, requestId: string): Promise<AuthThreadOpening['opening']> {
  try {
    return { status: 'ready', data: await threadOpening(world, reader, threadId) };
  } catch (error) {
    if (!(error instanceof Refusal) || error.answer.status === 401) throw error;
    const retry = new Headers(error.answer.headers).get('retry-after');
    return { status: 'failed', problem: { ...error.answer.body as Problem, request_id: requestId },
      retry_after: retry !== null && /^\d+$/.test(retry) ? Number(retry) : null };
  }
}

export async function threadOpening(world: World, person: Person, threadId: string, limit?: number): Promise<ThreadOpening> {
  async function read(path: string, query = new URLSearchParams()): Promise<unknown> {
    const search = query.toString();
    return (await world.requestFeeds.read(person, search === '' ? path : `${path}?${search}`)).body;
  }
  async function optional(path: string): Promise<unknown> {
    try { return await read(path); }
    catch (error) { if (error instanceof Refusal && error.answer.status === 403) return null; throw error; }
  }
  // Sync checks the thread's native admission before any of its related projections are read.
  const count = limit ?? (world.places.rowOf(threadId) === undefined ? 40 : Number(world.settings.valueAt(threadId, 'threads.opening_items').value));
  const sync = await read(`/threads/${threadId}/sync`, new URLSearchParams({ limit: String(count) })) as ThreadOpening['sync'];
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
    screen: { status: 'ready', data: world.threads.screenFacts(person, threadId) },
    draft: await optional(`/threads/${threadId}/draft`) as ThreadOpening['draft'],
    parameters: structuredClone(await optional(`/threads/${choices}/parameters`)) as ThreadOpening['parameters'],
    limits: await optional(`/threads/${choices}/limits`) as ThreadOpening['limits'],
    gateway: sync.projection?.follows_active === false ? null : {
      status: 'ready' as const, data: await read(`/threads/${threadId}/gateway`) as NonNullable<Extract<NonNullable<ThreadOpening['gateway']>, { status: 'ready' }>['data']>,
    },
    ...(pending === undefined ? {} : { pending_agent_schedules: pending }),
  };
  if (opening.parameters !== null) {
    const visibleTurns = new Set(sync.items.flatMap((item) => item.turn_id === null ? [] : [item.turn_id]));
    if (sync.thread.current_turn_id !== null) visibleTurns.add(sync.thread.current_turn_id);
    const models = new Set([opening.parameters.model.value, ...sync.turns.filter((turn) => visibleTurns.has(turn.id))
      .flatMap((turn) => [turn.model, ...(turn.usage_detail?.effective_model ? [turn.usage_detail.effective_model] : [])])]);
    opening.parameters.model.options = opening.parameters.model.options.filter((option) => models.has(option.model_name));
    for (const option of opening.parameters.model.options) {
      option.option_constraints = {};
      delete option.parameters.request_options;
      delete option.parameters.offered;
    }
    opening.parameters.model.options_partial = true;
    opening.parameters.approval_modes = null;
    opening.parameters.approval_reviewer = null;
    opening.parameters.subagent_limits = null;
    opening.parameters.port_tools = null;
  }
  return opening;
}
