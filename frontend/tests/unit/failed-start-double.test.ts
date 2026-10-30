// @vitest-environment node
/** The browser's fake route admits the same failed-unit retry as the native API, without resending input. */
import { expect, it } from 'vitest';
import { ACME, RUNS_ON } from '../doubles/fixtures';
import { threadInput } from '../doubles/thread-rows';
import { World } from '../doubles/world';

for (const status of ['none', 'stopped', 'failed'] as const) {
  it(`starts the ${status} unit and preserves its queued first input even with an attached machine`, () => {
    const world = new World();
    try {
      const person = world.people.get('ada');
      const record = world.resources.environments.find((entry) => entry.id === RUNS_ON.sandbox);
      if (person === undefined || record === undefined) throw new Error('the seeded person and unit must exist');
      record.status = status; record.machine = RUNS_ON.laptop;
      const input = { ...threadInput('first-input', 'keep this first message', { author: person.me, position: '1' }), thread_id: ACME.mainThread };
      world.threads.emit(ACME.mainThread, { inputs: [input] });
      const before = world.threads.snapshot(ACME.mainThread);
      const answer = world.resources.handle(person, 'POST', `/environments/${record.id}/start`, new URLSearchParams(), {});
      expect(answer).toMatchObject({ status: 202, body: { kind: 'environment_provision', environment_id: record.id, state: 'running', progress: { phase: 'files', phases: [], files_done: 0, eta_seconds: null } } });
      expect(record.status).toBe('provisioning');
      expect(world.threads.snapshot(ACME.mainThread)).toEqual(before);
    } finally { world.resources.reset(); }
  });
}

it('refuses to start a registered directory and retains the unit management check', () => {
  const world = new World();
  try {
    const person = world.people.get('ada');
    const reader = world.people.get('vera');
    const unit = world.resources.environments.find((entry) => entry.id === RUNS_ON.sandbox);
    const directory = world.resources.environments.find((entry) => entry.id === RUNS_ON.workspace);
    if (person === undefined || reader === undefined || unit === undefined || directory === undefined) throw new Error('the seeded people and environments must exist');
    directory.machine = null; directory.status = 'failed';
    expect(() => world.resources.handle(person, 'POST', `/environments/${directory.id}/start`, new URLSearchParams(), {})).toThrow(/request\.invalid/);
    unit.status = 'failed';
    expect(() => world.resources.handle(reader, 'POST', `/environments/${unit.id}/start`, new URLSearchParams(), {})).toThrow(/authz\.denied/);
    expect(unit.status).toBe('failed'); expect(directory.status).toBe('failed');
  } finally { world.resources.reset(); }
});
