/** The shell aggregate reuses the fake world's existing individual GET projections. */
import type { AdminMode, MeBootstrapReply } from '../../src/generated/api-types.ts';
import type { Person, World } from './world.ts';

export async function bootstrap(world: World, person: Person, authorization?: string): Promise<MeBootstrapReply> {
  const actor = authorization === undefined ? person : world.browserPerson(authorization);
  async function read<T>(path: string): Promise<T> {
    for (const { part } of world.parts) {
      const answer = await part.handle?.(actor, 'GET', path, new URLSearchParams(), {});
      if (answer !== null && answer !== undefined) return answer.body as T;
    }
    throw new Error(`No fake GET projection for ${path}`);
  }
  // The component fixture may project an unsigned identity. Every HTTP caller
  // supplies its bearer and shares the exact individual session-mode supplier.
  const admin: AdminMode = authorization === undefined
    ? { as_admin: false, available: world.adminModeAvailable(person) } : world.adminMode(authorization);
  return {
    meta: world.metaFor(authorization),
    person: person.me,
    state: await read<MeBootstrapReply['state']>('/me/state'),
    orgs: world.orgsOf(person),
    admin_mode: admin,
    favorites: await read<MeBootstrapReply['favorites']>('/me/favorites'),
    chat_states: await read<MeBootstrapReply['chat_states']>('/me/chat-states'),
    read_states: await read<NonNullable<MeBootstrapReply['read_states']>>('/me/read-states'),
    subscriptions: await read<NonNullable<MeBootstrapReply['subscriptions']>>('/me/subscriptions'),
    notification_summary: await read<MeBootstrapReply['notification_summary']>('/notifications/summary'),
  };
}
