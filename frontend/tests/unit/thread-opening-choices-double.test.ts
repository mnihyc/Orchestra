// @vitest-environment node
/** Opening a thread supplies the guarded choices required by its declared parameter controls. */
import { expect, it } from 'vitest';
import { ACME } from '../doubles/fixtures';
import { sessionThreadOpening, threadOpening } from '../doubles/thread-opening';
import { Refusal, World } from '../doubles/world';

it('leaves the full settings and outline to their own readers while supplying composer choices', async () => {
  const world = new World();
  const person = world.people.get('ada')!;
  const opening = await threadOpening(world, person, ACME.mainThread, 40);
  expect(opening).not.toHaveProperty('settings');
  expect(opening).not.toHaveProperty('outline');
  expect(opening.parameters?.composer?.plan_first).toBe(world.settings.valueAt(ACME.mainThread, 'thread.plan_first').value);
  expect(opening.parameters?.parameters.find((parameter) => parameter.key === 'verbosity')?.writable).toBe(true);
});

it('returns a declared choice after its guarded write and still refuses a viewer', async () => {
  const world = new World();
  const person = world.people.get('ada')!;
  const path = `/nodes/${ACME.mainThread}/settings/agent.params`;
  world.threads.handle(person, 'PUT', path, new URLSearchParams(), { value: { verbosity: 'terse' }, expected_revision: 0 });
  const opening = await threadOpening(world, person, ACME.mainThread, 200);
  expect(opening.parameters?.parameters.find((parameter) => parameter.key === 'verbosity')).toMatchObject({ value: 'terse', source: { scope: 'thread' } });
  const viewer = world.people.get('vera')!;
  expect(() => world.threads.handle(viewer, 'PUT', path, new URLSearchParams(), { value: { verbosity: 'chatty' }, expected_revision: 1 })).toThrow(Refusal);
  const readonly = await threadOpening(world, viewer, ACME.mainThread, 200);
  expect(readonly.parameters?.parameters.find((parameter) => parameter.key === 'verbosity')?.writable).toBe(false);
});


it('opens the addressed thread with the restored cookie and keeps a refused thread separate from sign-in', async () => {
  const world = new World();
  const login = world.login({ login: 'ada', password: world.people.get('ada')!.password });
  const cookie = login.cookies?.orch_session;
  if (typeof cookie !== 'string') throw new Error('Expected a session for the signed-in reader');
  const open = (cookies: Record<string, string>, csrf = '1') =>
    sessionThreadOpening(world, ACME.mainThread, cookies, csrf, 'opening-request');
  expect((await open({})).body).toBeNull();
  await expect(open({ orch_session: cookie }, '')).rejects.toBeInstanceOf(Refusal);
  expect((await open({ orch_session: cookie })).body).toMatchObject({
    session: { user: { username: 'ada' } }, opening: { status: 'ready', data: { sync: { thread: { id: ACME.mainThread } } } },
  });
  world.places.denied.set('ada', new Set(['thread.view']));
  expect((await open({ orch_session: cookie })).body).toMatchObject({
    session: { user: { username: 'ada' } }, opening: { status: 'failed', problem: { status: 403, request_id: 'opening-request' } },
  });
});
