/** Cold route/frame admission uses current bootstrap Meta; later native repairs remain available. */
import type * as AuthApi from '@/api/auth';
import { QueryObserver } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { metaKey } from '@/api/auth';
import { queryClient } from '@/api/query';
import { admissionMetaQuery } from '@/shell/admission';
import { admit } from '@/routes/-access';
import type { MeBootstrapReply, Meta } from '@/generated/api-types';
import { bootstrap } from '../doubles/bootstrap';
import { World } from '../doubles/world';

const calls = vi.hoisted(() => ({ bootstrap: vi.fn<() => Promise<MeBootstrapReply>>(), nativeMeta: vi.fn<() => Promise<Meta>>() }));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof AuthApi>(), fetchMeta: calls.nativeMeta }));
vi.mock('@/shell/bootstrap', async (loadOriginal) => ({ ...await loadOriginal<object>(), shellBootstrapKey: (id: string) => ['shell-bootstrap', id], ensureShellBootstrap: calls.bootstrap }));
const previous = client.session.getState();
afterEach(() => { queryClient.clear(); client.session.setState(previous); vi.restoreAllMocks(); });

it('joins the AuthFrame and root loader to one admitted aggregate without a public or authority pre-read', async () => {
  queryClient.clear(); calls.bootstrap.mockReset(); calls.nativeMeta.mockReset();
  const world = new World(); const data = await bootstrap(world, world.people.get('ada')!);
  client.session.setState({ status: 'signed_in', userId: data.person.id });
  vi.spyOn(client, 'restore').mockResolvedValue(true);
  let publish!: (data: MeBootstrapReply) => void;
  calls.bootstrap.mockImplementation(() => new Promise((resolve) => { publish = resolve; }));
  const frame = new QueryObserver(queryClient, admissionMetaQuery).subscribe(() => undefined);
  const admitted = admit({ needs: 'signed_in', href: '/org/acme', pathname: '/org/acme', onward: undefined });
  try {
    await vi.waitFor(() => { expect(calls.bootstrap).toHaveBeenCalledOnce(); });
    expect(calls.nativeMeta).not.toHaveBeenCalled(); expect(queryClient.getQueryData(metaKey)).toBeUndefined();
    publish(data); await admitted;
    expect(queryClient.getQueryData(metaKey)).toEqual(data.meta);
    expect(calls.bootstrap).toHaveBeenCalledOnce(); expect(calls.nativeMeta).not.toHaveBeenCalled();
  } finally { publish(data); frame(); }
});

it('keeps the native supplier for an anonymous entrance and a later explicit Meta repair', async () => {
  queryClient.clear(); calls.bootstrap.mockReset(); calls.nativeMeta.mockReset();
  const world = new World(); const data = await bootstrap(world, world.people.get('ada')!);
  const restore = vi.spyOn(client, 'restore').mockResolvedValue(false);
  calls.nativeMeta.mockResolvedValue(data.meta);
  await expect(queryClient.query(admissionMetaQuery)).resolves.toEqual(data.meta);
  expect(calls.bootstrap).not.toHaveBeenCalled(); expect(calls.nativeMeta).toHaveBeenCalledOnce();
  queryClient.removeQueries({ queryKey: metaKey });
  client.session.setState({ status: 'signed_in', userId: data.person.id }); restore.mockResolvedValue(true);
  queryClient.setQueryData(['shell-bootstrap', data.person.id], data);
  await expect(queryClient.query(admissionMetaQuery)).resolves.toEqual(data.meta);
  expect(calls.bootstrap).not.toHaveBeenCalled(); expect(calls.nativeMeta).toHaveBeenCalledTimes(2);
});
