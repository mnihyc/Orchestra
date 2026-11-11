/** The actual transport renews a refused creation with the same key and body. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it } from 'vitest';
import { accepted, createApiClient, repeatable } from '@/api/client';
import type { AuthResult } from '@/generated/api-types';
import { newActKey } from '@/lib/requests';
import { me } from '../doubles/fixtures';
import { show } from './support';

const clients: ReturnType<typeof createApiClient>[] = [];
const grant = (token: string): AuthResult => ({ access_token: token,
  access_expires_at: new Date(Date.now() + 900_000).toISOString(), user: me(1, 'creator', 'Creator') });
afterEach(async () => { cleanup(); await Promise.all(clients.splice(0).map((client) => client.signOut())); localStorage.clear(); });

for (const kind of ['thread', 'fork', 'document'] as const) {
  it(`keeps one ${kind} attempt through renewal and gives the next attempt a fresh UUID`, async () => {
    const seen: { key: string | null; body: string; bearer: string | null }[] = [];
    const client = createApiClient({ baseUrl: 'http://localhost/api/v1', fetch: async (input) => {
      const request = input instanceof Request ? input : new Request(input);
      if (new URL(request.url).pathname.endsWith('/auth/refresh')) return Response.json(grant('renewed'));
      if (new URL(request.url).pathname.endsWith('/auth/logout')) return new Response(null, { status: 204 });
      seen.push({ key: request.headers.get('Idempotency-Key'), body: await request.text(), bearer: request.headers.get('Authorization') });
      if (request.headers.get('Authorization') === 'Bearer first') return Response.json({
        type: 'orch:error:auth.invalid_token', code: 'auth.invalid_token', status: 401, title: 'Renew', detail: '', request_id: 'retry',
      }, { status: 401 });
      return Response.json({ id: 'accepted-identity' }, { status: kind === 'document' ? 201 : kind === 'fork' ? 202 : 200 });
    } });
    clients.push(client); client.adopt(grant('first'));
    const create = (key: string): Promise<unknown> => {
      const headers = repeatable(key);
      if (kind === 'thread') return accepted(client.api.POST('/topics/{id}/threads', { params: { path: { id: 'topic' } }, body: { name: 'One attempt' }, ...headers }));
      if (kind === 'fork') return accepted(client.api.POST('/threads/{id}/forks/now', { params: { path: { id: 'source' } }, body: { name: 'One attempt' }, ...headers }));
      return accepted(client.api.POST('/nodes/{id}/documents', { params: { path: { id: 'topic' } }, body: { title: 'One attempt' }, ...headers }));
    };
    function Attempt() {
      const [pending, setPending] = useState(false);
      return <button disabled={pending} onClick={() => { setPending(true); void create(newActKey()).finally(() => setPending(false)); }}>Create</button>;
    }
    show(<Attempt />);
    await userEvent.click(screen.getByRole('button', { name: 'Create' }));
    await waitFor(() => expect(seen).toHaveLength(2));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Create' })).toBeEnabled());
    expect(seen[0]?.key).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i);
    expect(seen[1]).toEqual({ ...seen[0], bearer: 'Bearer renewed' });
    await userEvent.click(screen.getByRole('button', { name: 'Create' }));
    await waitFor(() => expect(seen).toHaveLength(3));
    expect(seen[2]?.key).not.toBe(seen[0]?.key);
    expect(seen[2]?.body).toBe(seen[0]?.body);
  });
}
