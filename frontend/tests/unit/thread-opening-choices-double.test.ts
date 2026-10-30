// @vitest-environment node
/** Opening a thread supplies the guarded choices required by its declared parameter controls. */
import { expect, it } from 'vitest';
import { ACME } from '../doubles/fixtures';
import { threadOpening } from '../doubles/thread-opening';
import { Refusal, World } from '../doubles/world';

it('includes the same agent parameter owner as the thread settings reader', async () => {
  const world = new World();
  const person = world.people.get('ada')!;
  const opening = await threadOpening(world, person, ACME.mainThread, 200);
  const owner = opening.settings.find((setting) => setting.key === 'agent.params');
  expect(owner).toEqual(world.threads.settingsOf(ACME.mainThread).find((setting) => setting.key === 'agent.params'));
  expect(owner?.writable).toBe(true);
  expect(opening.parameters?.parameters.find((parameter) => parameter.key === 'verbosity')?.writable).toBe(true);
  expect(opening.settings.some((setting) => setting.key === 'usage.rates')).toBe(true);
  expect(opening.settings.some((setting) => setting.key === 'docs.templates')).toBe(true);
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
  expect(readonly.settings.find((setting) => setting.key === 'agent.params')?.writable).toBe(false);
});
