// @vitest-environment node
/** A started environment's returned job remains readable until the same operation finishes. */
import { expect, it, vi } from 'vitest';
import type { Environment, Job } from '@/generated/api-types';
import { RUNS_ON } from '../doubles/fixtures';
import { Refusal, World } from '../doubles/world';

it('serves the returned start job and completes it when the environment reaches Running', async () => {
  vi.useFakeTimers();
  try {
    const world = new World();
    const person = world.people.get('ada')!;
    const answer = world.resources.handle(person, 'POST', `/environments/${RUNS_ON.sandbox}/start`, new URLSearchParams(), {});
    if (answer === null) throw new Error('The environment did not start');
    const job = answer.body as Job;
    expect(job.state).toBe('running');
    expect((await world.requestFeeds.read(person, `/jobs/${job.id}`)).body).toMatchObject({ id: job.id, environment_id: RUNS_ON.sandbox, state: 'running' });
    await vi.runAllTimersAsync();
    expect((await world.requestFeeds.read(person, `/jobs/${job.id}`)).body).toMatchObject({ id: job.id, state: 'succeeded' });
    const environment = (await world.requestFeeds.read(person, `/environments/${RUNS_ON.sandbox}`)).body as Environment;
    expect(environment.status).toBe('running');
  } finally { vi.useRealTimers(); }
});

it('still refuses Start to a viewer who does not manage the environment', () => {
  const world = new World();
  const viewer = world.people.get('vera')!;
  expect(() => world.resources.handle(viewer, 'POST', `/environments/${RUNS_ON.sandbox}/start`, new URLSearchParams(), {})).toThrow(Refusal);
});
