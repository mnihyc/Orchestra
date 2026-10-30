/** A request belongs to the session that sent it, including an answer arriving after another sign-in. */
import { expect, it, vi } from 'vitest';
import { createApiClient } from '@/api/client';
import { World } from '../doubles/world';

it.each([200, 401])('does not apply or replay an old session’s write after a new sign-in (%s)', async (status) => {
  vi.useFakeTimers();
  try {
    const world = new World();
    const ada = world.people.get('ada')?.me, dora = world.people.get('dora')?.me;
    if (ada === undefined || dora === undefined) throw new Error('the native world supplies both people');
    let finish: (reply: Response) => void = () => { throw new Error('the reply is not waiting'); };
    let notice: () => void = () => undefined;
    const response = new Promise<Response>((resolve) => { finish = resolve; });
    const sent = new Promise<void>((resolve) => { notice = resolve; });
    const fetch = vi.fn(() => { notice(); return response; });
    const client = createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch });
    const expiry = new Date(Date.now() + 900_000).toISOString();
    client.adopt({ user: ada, access_token: 'ada-token', access_expires_at: expiry });
    const changed = vi.fn(); client.onAct(changed);
    const pending = client.api.PATCH('/me', { body: { display_name: 'Changed by Ada' } });
    await sent;
    client.adopt({ user: dora, access_token: 'dora-token', access_expires_at: expiry });
    finish(Response.json(status === 200 ? { ...ada, display_name: 'Changed by Ada' } : { type: 'orch:error:auth.invalid_token', code: 'auth.invalid_token', title: '', detail: '', status, request_id: 'old-request' }, { status }));
    const answer = await pending;
    expect(answer.response.status).toBe(status);
    expect(client.session.getState().userId).toBe(dora.id);
    expect(changed).not.toHaveBeenCalled();
    expect(fetch).toHaveBeenCalledTimes(1);
  } finally { vi.useRealTimers(); }
});
