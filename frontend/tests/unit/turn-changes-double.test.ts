// @vitest-environment node
/** The workspace double filters by native item/turn provenance, just like the SQL reader. */
import { IncomingMessage } from 'node:http';
import { Socket } from 'node:net';
import { expect, it } from 'vitest';
import { World } from '../doubles/world';
import { ACME } from '../doubles/fixtures';
import { AT, change, fileChange, turn } from '../doubles/thread-rows';

it('returns the selected turn and no other turn or invented turn', async () => {
  const world = new World(), person = world.people.get('ada'), place = world.places.rowOf(ACME.mainThread);
  if (person === undefined || place === undefined) throw new Error('The fixture needs its ordinary native owner');
  const thread = world.threads.thread(person, place); const environment = thread.environment;
  if (environment === null) throw new Error('The file changes need their actual environment');
  const first = fileChange('first-change', { turn: 'first-turn', changes: [change('web/a.ts', '@@ -1 +1 @@\n-old\n+first\n')] });
  const second = fileChange('second-change', { turn: 'second-turn', changes: [change('web/b.ts', '@@ -1 +1 @@\n-old\n+second\n')] });
  world.threads.emit(thread.id, { items: [first, second], turns: [turn('first-turn', { ordinal: 1, state: 'completed', inputId: 'first-input' }), turn('second-turn', { ordinal: 2, state: 'completed', inputId: 'second-input' })] });
  const rows = [first, second].flatMap((item) => item.kind === 'tool_call' && item.payload.tool.kind === 'file_change' ? item.payload.tool.changes.map((file) => ({ ...file, thread_id: thread.id, item_id: item.id, at: AT })) : []);
  world.workspace.changes.set(environment.id, rows);
  const socket = new Socket(); const request = new IncomingMessage(socket);
  try {
    const read = (id: string) => world.workspace.raw(person, request, 'GET', `/environments/${environment.id}/changes`, new URLSearchParams({ thread: thread.id, turn_id: id }));
    expect((await read('first-turn'))?.body).toEqual({ changes: [rows[0]] });
    expect((await read('second-turn'))?.body).toEqual({ changes: [rows[1]] });
    expect((await read('not-a-turn'))?.body).toEqual({ changes: [] });
    const exact = await world.workspace.raw(person, request, 'GET', `/environments/${environment.id}/changes`, new URLSearchParams({ thread: thread.id, item_id: first.id }));
    expect(exact?.body).toEqual({ changes: [rows[0]] });
    const mismatch = await world.workspace.raw(person, request, 'GET', `/environments/${environment.id}/changes`, new URLSearchParams({ thread: thread.id, turn_id: 'second-turn', item_id: first.id }));
    expect(mismatch?.body).toEqual({ changes: [] });
  } finally { socket.destroy(); }
});

it('keeps an older exact patch after a newer item changed the same path', async () => {
  const world = new World(), person = world.people.get('ada'), place = world.places.rowOf(ACME.mainThread);
  if (person === undefined || place === undefined) throw new Error('The fixture needs its native file reader');
  const thread = world.threads.thread(person, place); const environment = thread.environment;
  if (environment === null) throw new Error('The recorded paths belong to an environment');
  const first = { ...change('web/a.ts', '@@ -1 +1 @@\n-old\n+first\n'), item_id: 'first-edit', thread_id: thread.id, at: AT };
  const later = { ...first, item_id: 'later-edit', diff: '@@ -1 +1 @@\n-first\n+later\n', at: '2026-10-01T10:01:00.000Z' };
  world.workspace.changes.set(environment.id, [first, later]);
  const socket = new Socket(); const request = new IncomingMessage(socket);
  try {
    const read = (item?: string) => world.workspace.raw(person, request, 'GET', `/environments/${environment.id}/changes`, new URLSearchParams({ thread: thread.id, ...(item === undefined ? {} : { item_id: item }) }));
    expect((await read())?.body).toEqual({ changes: [later] });
    expect((await read(first.item_id))?.body).toEqual({ changes: [first] });
    expect((await read('missing-edit'))?.body).toEqual({ changes: [] });
  } finally { socket.destroy(); }
});
