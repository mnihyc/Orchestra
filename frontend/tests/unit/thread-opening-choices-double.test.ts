// @vitest-environment node
/** Opening a thread supplies the guarded choices required by its declared parameter controls. */
import { expect, it } from 'vitest';
import { ACME } from '../doubles/fixtures';
import { threadOpening } from '../doubles/thread-opening';
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
