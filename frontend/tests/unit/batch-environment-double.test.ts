// @vitest-environment node
/** The browser double preserves the native one-environment batch contract. */
import { expect, it } from 'vitest';
import type { ForkBatch, ForkBatchPlan } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';

for (const cancel of [false, true]) {
  it(cancel ? 'cleans the one unpublished target after every member is cancelled' : 'moves both threads into one new environment', () => {
    const world = new World();
    world.change.control('/mode', { mode: 'manual' });
    try {
      const person = world.people.get('ada');
      if (person === undefined) throw new Error('the seeded manager must exist');
      const threads = [ACME.mainThread, ACME.depsThread];
      const before = threads.map((thread) => world.resources.bindingOf(thread));
      const environments = world.resources.environments.length;
      const preview = world.change.handle(person, 'POST', '/forks/preflight', new URLSearchParams(), {
        thread_ids: threads, environment: { new: { name: 'Shared batch workspace' } },
      })?.body as ForkBatchPlan;
      expect(preview.executable).toBe(2);
      expect(preview.plans.every((plan) => plan.blockers.length === 0)).toBe(true);
      expect(world.resources.environments).toHaveLength(environments);
      const started = world.change.handle(person, 'POST', '/forks', new URLSearchParams(), { preflight_id: preview.preflight_id })?.body as ForkBatch;
      expect(started.jobs).toHaveLength(2);
      // Both jobs reach their provisioning without a second target appearing.
      world.change.step(null, 1);
      expect(world.resources.environments).toHaveLength(environments + 1);
      if (cancel) world.change.handle(person, 'POST', `/forks/${started.batch_id}/cancel`, new URLSearchParams(), {});
      world.change.finish(null);
      if (cancel) {
        expect(world.resources.environments).toHaveLength(environments);
        expect(threads.map((thread) => world.resources.bindingOf(thread))).toEqual(before);
      } else {
        const targets = threads.map((thread) => world.resources.bindingOf(thread));
        expect(new Set(targets.map((target) => target.environment)).size).toBe(1);
        expect(world.resources.environments.find((environment) => environment.id === targets[0]?.environment)?.name).toBe('Shared batch workspace');
        expect(world.resources.environments).toHaveLength(environments + 1);
      }
    } finally { world.change.reset(); }
  });
}
