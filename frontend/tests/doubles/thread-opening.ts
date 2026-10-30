/** The first thread screen reuses the fake world's native readers without making nested HTTP requests. */
import type { ThreadOpening } from '../../src/generated/api-types.ts';
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
  return {
    sync,
    draft: await optional(`/threads/${threadId}/draft`) as ThreadOpening['draft'],
    parameters: await optional(`/threads/${choices}/parameters`) as ThreadOpening['parameters'],
    limits: await optional(`/threads/${choices}/limits`) as ThreadOpening['limits'],
    settings: await read(`/nodes/${choices}/settings`) as ThreadOpening['settings'],
    outline: parent === undefined
      ? await read(`/threads/${threadId}/outline`, new URLSearchParams({ limit: String(limit) })) as ThreadOpening['outline']
      : world.workspace.outlineSnapshot(sync, new URLSearchParams({ limit: String(limit) })),
  };
}
