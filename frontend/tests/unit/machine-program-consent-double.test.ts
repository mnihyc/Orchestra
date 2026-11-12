// @vitest-environment node
/** Machine reports, policy handover and environment consent use the same typed HTTP/native projection. */
import { expect, it, vi } from 'vitest';
import { RUNS_ON, me } from '../doubles/fixtures.ts';
import { Refusal, World, type Answer, type Person, type Subscriber } from '../doubles/world.ts';

const KEY = 'exec.version_overrides';
const program = { kind: 'codex', path: '/usr/local/bin/codex', version: '0.164.0', daemon: true };
const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
function object(value: unknown): Record<string, unknown> {
  if (!isRecord(value)) throw new Error('The native fixture must return an object');
  return value;
}
function objects(value: unknown): Record<string, unknown>[] {
  if (!Array.isArray(value)) throw new Error('The native fixture must return an array');
  return value.map(object);
}
function ok(answer: Answer | null): Record<string, unknown> {
  expect(answer?.status).toBe(200);
  return object(answer?.body);
}
function setup(confirmed = false) {
  const world = new World();
  const auth = ok(world.login({ login: 'ada', password: 'correct horse battery' }));
  if (typeof auth.access_token !== 'string') throw new Error('The fixture needs its real browser session');
  const actor = world.browserPerson(`Bearer ${auth.access_token}`);
  const machine = world.resources.machines.find((row) => row.id === RUNS_ON.laptop);
  const environment = world.resources.environments.find((row) => row.id === RUNS_ON.workspace);
  if (machine === undefined || environment === undefined) throw new Error('The established machine and directory fixtures are required');
  machine.roles = ['environment'];
  machine.harnesses = confirmed ? [program] : [];
  Object.assign(machine, { discoveredHarnesses: confirmed ? [] : [program], searchedHarnessKinds: ['codex'] });
  machine.policy = { ...machine.policy, 'agent.harnesses': confirmed ? [{ kind: program.kind, path: program.path }] : [], 'policy.delegated': ['agent.harnesses'], 'policy.local': {} };
  environment.machine = machine.id; environment.host = null; environment.template = null;
  environment.config = { 'env.variables': { KEEP: 'unchanged' } };
  return { world, actor, machine, environment };
}
function readMachine(state: ReturnType<typeof setup>) {
  return ok(state.world.resources.handle(state.actor, 'GET', `/machines/${state.machine.id}`, new URLSearchParams(), {}));
}
function change(state: ReturnType<typeof setup>, action: 'confirm_harness' | 'remove_harness' | 'reload') {
  return ok(state.world.resources.handle(state.actor, 'POST', `/machines/${state.machine.id}/programs`, new URLSearchParams(), action === 'reload' ? { action } : { action, kind: program.kind, path: program.path }));
}
function patch(state: ReturnType<typeof setup>, config: Record<string, unknown>, actor: Person = state.actor) {
  return ok(state.world.resources.handle(actor, 'PATCH', `/environments/${state.environment.id}`, new URLSearchParams(), { expected_revision: state.environment.revision, config }));
}
function refusal(run: () => unknown, status: number, code: string) {
  let caught: unknown;
  try { run(); } catch (error) { caught = error; }
  expect(caught).toBeInstanceOf(Refusal);
  if (!(caught instanceof Refusal)) throw new Error('The actual fixture mutation must refuse this request');
  expect(caught.answer.status).toBe(status); expect(object(caught.answer.body).code).toBe(code);
  return object(caught.answer.body);
}

it('distinguishes reported discoveries, searched-empty and reset-to-unreported facts', () => {
  const state = setup();
  expect(object(readMachine(state).facts)).toMatchObject({ harnesses: [], discovered_harnesses: [program], searched_harness_kinds: ['codex'] });
  Object.assign(state.machine, { discoveredHarnesses: [], searchedHarnessKinds: ['codex'] });
  expect(object(readMachine(state).facts)).toMatchObject({ discovered_harnesses: [], searched_harness_kinds: ['codex'] });
  state.world.resources.reset();
  const reset = object(readMachine(state).facts);
  expect(reset.discovered_harnesses).toBeUndefined(); expect(reset.searched_harness_kinds).toBeUndefined();
});

it('applies harness policy to an Environment-only role and keeps native actions separate from the registry', () => {
  const state = setup();
  const policy = ok(state.world.resources.handle(state.actor, 'GET', `/machines/${state.machine.id}/policy`, new URLSearchParams(), {}));
  const key = objects(policy.keys).find((entry) => entry.key === 'agent.harnesses');
  expect(key).toMatchObject({ definition: { sections: ['environment', 'agent'] }, delegated: true, writable: true });
  expect(objects(policy.keys).some((entry) => entry.key === 'agent.programs' || entry.key === 'tools.fetch')).toBe(false);
  const confirmed = change(state, 'confirm_harness');
  expect(confirmed.entries).toEqual([]);
  expect(object(object(confirmed.machine_view).facts)).toMatchObject({ harnesses: [program], discovered_harnesses: [] });
  expect(state.machine.policy['agent.harnesses']).toEqual([{ kind: program.kind, path: program.path }]);
  expect(change(state, 'reload').entries).toEqual([]);
  const removed = change(state, 'remove_harness');
  expect(state.machine.policy['agent.harnesses']).toEqual([]);
  expect(object(object(removed.machine_view).facts)).toMatchObject({ harnesses: [], discovered_harnesses: [program] });
});

it('accepts exact or Agent handover while rejecting Environment handover and a local veto', () => {
  for (const handed of ['agent.harnesses', 'agent.*', '*']) {
    const state = setup(); state.machine.policy['policy.delegated'] = [handed];
    expect(object(object(change(state, 'confirm_harness').machine_view).facts).harnesses).toEqual([program]);
  }
  for (const policy of [{ 'policy.delegated': ['environment.*'] }, { 'policy.delegated': ['agent.programs'] }, { 'policy.delegated': ['*'], 'policy.local': { 'agent.harnesses': [] } }]) {
    const state = setup(); Object.assign(state.machine.policy, policy);
    const refused = refusal(() => change(state, 'confirm_harness'), 409, 'policy.not_delegated');
    expect(refused.keys).toEqual(['agent.harnesses']); expect(state.machine.harnesses).toEqual([]);
  }
});

it('uses Agent-only section handover consistently for policy display and writes', () => {
  const state = setup(); state.machine.policy['policy.delegated'] = ['environment.*'];
  const policy = ok(state.world.resources.handle(state.actor, 'GET', `/machines/${state.machine.id}/policy`, new URLSearchParams(), {}));
  expect(objects(policy.keys).find((entry) => entry.key === 'agent.harnesses')).toMatchObject({ delegated: false, writable: false });
  const denied = refusal(() => state.world.resources.handle(state.actor, 'PATCH', `/machines/${state.machine.id}/policy`, new URLSearchParams(), { 'agent.harnesses': [] }), 409, 'policy.not_delegated');
  expect(denied.keys).toEqual(['agent.harnesses']);
});

it('stamps consent on the server, preserves it on a no-op, and mirrors current and stale tuples over the native feed', async () => {
  const state = setup(true), { world, actor, environment, machine } = state;
  const frames: unknown[] = [];
  const subscriber: Subscriber = { send: (frame) => { frames.push(frame); }, close: vi.fn() };
  expect(world.users.frame(subscriber, actor, { op: 'sub', id: 3, feed: 'user' })).toBe(true);
  const before = ok(await world.requestFeeds.read(actor, `/environments/${environment.id}`));
  expect(object(before.exec).machine_programs).toEqual([{ harness_kind: 'codex', path: program.path, version: program.version, approval: null }]);
  const saved = patch(state, { ...environment.config, [KEY]: { codex: { path: program.path, version: program.version, by: me(999, 'forged-author', 'Forged author').id, at: '2000-01-01T00:00:00Z' } } });
  const stored = object(object(saved.config)[KEY]).codex;
  expect(stored).toMatchObject({ path: program.path, version: program.version, by: actor.me.id });
  expect(object(stored).at).not.toBe('2000-01-01T00:00:00Z');
  const same = patch(state, { ...environment.config, [KEY]: { codex: { path: program.path, version: program.version } } });
  expect(object(object(same.config)[KEY]).codex).toEqual(stored);
  frames.length = 0; await world.requestFeeds.changed();
  const rows = frames.flatMap((value) => { const frame = object(value); const data = object(frame.data); return data.what === 'resources' && data.kind === 'environment' && data.id === environment.id ? [object(data.row)] : []; });
  expect(rows).toHaveLength(1); expect(object(rows[0]?.exec).machine_programs).toEqual(object(saved.exec).machine_programs);
  machine.harnesses = [{ ...program, version: 'unknown' }];
  const stale = ok(await world.requestFeeds.read(actor, `/environments/${environment.id}`));
  expect(object(stale.exec).machine_programs).toEqual([{ harness_kind: 'codex', path: program.path, version: 'unknown', approval: null }]);
  const off = patch(state, { ...environment.config, [KEY]: {} });
  expect(object(off.config)['env.variables']).toEqual({ KEEP: 'unchanged' }); expect(objects(object(off.exec).machine_programs)[0]).toMatchObject({ approval: null });
  world.users.gone(subscriber);
});

it('rejects unknown consent members and refuses a reader without Manage', () => {
  for (const extra of [{ hash: 'not-a-consent-field' }, { agent_build: '0.165.0' }]) {
    const state = setup(true), prior = structuredClone(state.environment.config);
    refusal(() => patch(state, { ...state.environment.config, [KEY]: { codex: { path: program.path, version: program.version, ...extra } } }), 400, 'request.invalid');
    expect(state.environment.config).toEqual(prior);
  }
  const state = setup(true), viewer = state.world.people.get('vera');
  if (viewer === undefined) throw new Error('The established Viewer fixture is required');
  refusal(() => patch(state, { ...state.environment.config, [KEY]: { codex: { path: program.path, version: program.version } } }, viewer), 403, 'authz.denied');
});

it('does not replace a managed template cache with a machine program', async () => {
  const state = setup(true), template = state.world.resources.templates.find((row) => row.kind === 'environment');
  if (template === undefined) throw new Error('The established template fixture is required');
  state.environment.template = template.id; state.environment.machine = null;
  const row = ok(await state.world.requestFeeds.read(state.actor, `/environments/${state.environment.id}`));
  expect(object(row.exec).machine_programs).toBeUndefined();
});

it('refuses a service manager’s meaningful consent change without affecting other values', () => {
  const state = setup(true);
  const service = state.world.addPerson(me(30, 'ci-robot', 'CI robot', { kind: 'service' }), 'synthetic');
  state.environment.manages.push(service.me.username);
  const before = structuredClone(state.environment.config);
  refusal(() => patch(state, { ...state.environment.config, [KEY]: { codex: { path: program.path, version: program.version } } }, service), 403, 'authz.denied');
  expect(state.environment.config).toEqual(before);
});
