// @vitest-environment node
/** Chat guests keep only chat authority, and atomic forwarding identifies the actual refused destination. */
import { expect, it } from 'vitest';
import { ACME } from '../doubles/fixtures';
import { Refusal, World } from '../doubles/world';

it('lets an admitted guest send without granting access to the organization', () => {
  const world = new World();
  world.chats.control('/guest', { username: 'gus' });
  const made = world.chats.control('/chat', { org: ACME.org, members: ['ada', 'dora'], name: 'Guest review', guests: ['gus'] });
  const id = (made.body as { chat: string }).chat, guest = world.people.get('gus');
  if (guest === undefined) throw new Error('The admitted guest is required');
  const answer = world.peopleWork.handle(guest, 'POST', `/chats/${id}/messages`, new URLSearchParams(), { client_id: 'guest-boundary', parts: [{ type: 'text', text: 'Only this chat' }], reply_to: null });
  expect(answer?.status).toBe(201);
  expect(() => world.places.node(guest, ACME.org)).toThrow(Refusal);
});

it('names the refused chat during prevalidation and writes nothing to an allowed target', () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The forwarding reader is required');
  world.chats.control('/seed', {});
  const made = world.chats.control('/chat', { org: ACME.org, members: ['ada', 'dora'], name: 'Allowed target' });
  const allowed = world.chats.records.get((made.body as { chat: string }).chat);
  world.chats.control('/switch', { node: 'Upgrade deps', off: true });
  const origin = [...world.chats.records.values()].find((row) => row.nodeId === ACME.fixLogin);
  const refused = [...world.chats.records.values()].find((row) => row.nodeId === ACME.upgradeDeps);
  const message = origin?.messages.find((row) => row.kind === 'message');
  if (allowed === undefined || origin === undefined || refused === undefined || message === undefined) throw new Error('The native forwarding scene is required');
  const before = allowed.messages.length;
  try {
    world.chats.handle(person, 'POST', '/forward', new URLSearchParams(), { from: { chat_id: origin.id, message_ids: [message.id] }, to: [{ chat_id: allowed.id }, { chat_id: refused.id }], client_id: 'atomic-destination' });
    expect.fail('The disabled destination must refuse the complete forward');
  } catch (error) {
    expect(error).toBeInstanceOf(Refusal);
    if (!(error instanceof Refusal)) throw error;
    expect(error.answer.body).toMatchObject({ object: { chat_id: refused.id } });
  }
  expect(allowed.messages).toHaveLength(before);
});
