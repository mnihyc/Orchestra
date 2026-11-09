// @vitest-environment node
/** Environment build observations preserve confirmed/used provenance and current View authority. */
import { expect, it } from 'vitest';
import { ACME, RUNS_ON, me } from '../doubles/fixtures.ts';
import { Refusal, World } from '../doubles/world.ts';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
function setup() {
  const world = new World(), actor = world.people.get('ada');
  const machine = world.resources.machines.find((row) => row.id === RUNS_ON.laptop);
  const environment = world.resources.environments.find((row) => row.id === RUNS_ON.workspace);
  const binding = world.resources.bindings.get(ACME.mainThread);
  if (actor === undefined || machine === undefined || environment === undefined || binding === undefined) throw new Error('Established resource fixtures are required');
  environment.machine = machine.id; environment.template = null;
  machine.harnesses = [
    { kind: 'codex', path: '/first/codex', version: '10.0.0', daemon: true },
    { kind: 'codex', path: '/second/codex', version: '2.0.0', daemon: true },
    { kind: 'claude-code', path: '/usr/local/bin/claude', version: '2.1.296', daemon: false },
  ];
  machine.policy['agent.harnesses'] = [{ kind: 'codex', path: '/first/codex' }, { kind: 'codex', path: '/second/codex' }, { kind: 'claude-code', path: '/usr/local/bin/claude' }];
  return { world, actor, machine, environment, binding };
}

it('unions confirmed and used pairs in deterministic order without inventing held build identity', () => {
  const state = setup(), { world, actor, machine, environment, binding } = state;
  const agent = world.resources.agents.find((row) => row.id === binding.agent), provider = world.resources.providers.find((row) => row.id === binding.provider);
  if (agent === undefined || provider === undefined) throw new Error('The native binding needs its existing provider and agent');
  provider.machine = machine.id; provider.agent = agent.id;
  expect(world.resources.control('/provider-program-report', { id: provider.id, state: 'online' })?.status).toBe(200);
  Object.assign(machine, { discoveredHarnesses: [{ kind: 'codex', path: '/found/codex', version: '999.0.0', daemon: true }] });
  const answer = world.resources.handle(actor, 'GET', `/environments/${environment.id}/harnesses`, new URLSearchParams(), {});
  expect(answer?.status).toBe(200);
  if (!Array.isArray(answer?.body)) throw new Error('The declared RunningHarnessBuild list is required');
  const rows = answer.body.filter(record);
  const expected = new Map([['claude-code:2.1.296', ['claude-code', '2.1.296']], ['codex:10.0.0', ['codex', '10.0.0']], [`${agent.harness}:0.162.1`, [agent.harness, '0.162.1']]]);
  const pairs = [...expected.values()].sort(([ak = '', av = ''], [bk = '', bv = '']) => ak < bk ? -1 : ak > bk ? 1 : av < bv ? -1 : av > bv ? 1 : 0);
  expect(rows.map((row) => [row.kind, row.version])).toEqual(pairs);
  expect(rows.every((row) => row.build === null)).toBe(true);
  expect(rows.every((row) => record(row.harness) && typeof row.harness.name === 'string')).toBe(true);
  expect(rows.some((row) => row.version === '2.0.0' || row.version === '999.0.0')).toBe(false);
});

it('requires current Environment View and leaves managed-cache observations uninvented', () => {
  const { world, actor, environment } = setup();
  const template = world.resources.templates.find((row) => row.kind === 'environment');
  if (template === undefined) throw new Error('The established template is required');
  environment.machine = null; environment.template = template.id;
  const answer = world.resources.handle(actor, 'GET', `/environments/${environment.id}/harnesses`, new URLSearchParams(), {});
  expect(answer).toMatchObject({ status: 200, body: [] });
  const outsider = world.addPerson(me(77, 'outside-reader', 'Outside reader'), 'synthetic');
  expect(() => world.resources.handle(outsider, 'GET', `/environments/${environment.id}/harnesses`, new URLSearchParams(), {})).toThrow(Refusal);
});
