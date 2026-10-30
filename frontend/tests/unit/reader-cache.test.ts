/** A direct change of person retires the same private cache as sign-out. */
import { QueryClient } from '@tanstack/react-query';
import { expect, it } from 'vitest';
import { metaKey } from '@/api/auth';
import { createApiClient } from '@/api/client';
import { installReaderCacheBoundary } from '@/api/readerCache';
import type { AuthResult } from '@/generated/api-types';
import { me } from '../doubles/fixtures';

const grant = (id: number, username: string): AuthResult => ({
  user: me(id, username, username), access_token: `test-${username}`, access_expires_at: new Date(Date.now() + 900_000).toISOString(),
});

it('keeps same-person renewals but clears private data on a direct account change and sign-out', async () => {
  const cache = new QueryClient();
  const client = createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: () => Promise.resolve(new Response(null, { status: 204 })) });
  const stop = installReaderCacheBoundary(client, cache);
  try {
    client.adopt(grant(1, 'first'));
    cache.setQueryData(metaKey, { may_create_org: true, name: 'Public' });
    cache.setQueryData(['reference-preview', 'document', 'private'], 'Private detail');
    client.adopt(grant(1, 'first'));
    expect(cache.getQueryData(['reference-preview', 'document', 'private'])).toBe('Private detail');
    client.adopt(grant(2, 'second'));
    expect(cache.getQueryData(['reference-preview', 'document', 'private'])).toBeUndefined();
    expect(cache.getQueryData(metaKey)).toMatchObject({ name: 'Public', may_create_org: false });
    cache.setQueryData(['private-second'], 'Second person');
    await client.signOut();
    expect(cache.getQueryData(['private-second'])).toBeUndefined();
    expect(cache.getQueryData(metaKey)).toMatchObject({ name: 'Public', may_create_org: false });
  } finally { stop(); cache.clear(); }
});

it('does not restore a previous reader’s result when a request finishes late', async () => {
  const cache = new QueryClient();
  const client = createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: () => Promise.resolve(new Response(null, { status: 204 })) });
  const stop = installReaderCacheBoundary(client, cache);
  try {
    client.adopt(grant(1, 'first'));
    let finish: (value: string) => void = () => { throw new Error('Read was not started'); };
    const pending = cache.query({ queryKey: ['private'], queryFn: () => new Promise<string>((resolve) => { finish = resolve; }) });
    client.adopt(grant(2, 'second'));
    finish('Late private data');
    await pending.catch(() => undefined);
    expect(cache.getQueryData(['private'])).toBeUndefined();
    await client.signOut();
  } finally { stop(); cache.clear(); }
});
