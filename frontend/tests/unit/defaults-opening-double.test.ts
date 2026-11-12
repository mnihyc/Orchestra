// @vitest-environment node
/** Opted-in Defaults in the browser double reuse the same per-reader native suppliers and refusals. */
import { expect, it, vi } from 'vitest';
import type { NodeDefaultsOpening, ResolvedSetting } from '@/generated/api-types';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { ACME } from '../doubles/fixtures';
import { nodeChatOpening } from '../doubles/screen-openings';
import { Refusal, World, type Person } from '../doubles/world';

const id = ACME.fixLogin, providerId = '00000000-0000-4000-8022-000000000001';
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
function reader(world: World): Person {
  const answer = world.login({ login: 'ada', password: 'correct horse battery' });
  if (!record(answer.body) || typeof answer.body.access_token !== 'string') throw new Error('The fixture needs its actual browser reader');
  return world.browserPerson(`Bearer ${answer.body.access_token}`);
}
function selected(world: World, person: Person) {
  world.settings.handle(person, 'PUT', `/nodes/${id}/settings/agent.default_provider_id`, new URLSearchParams(), { value: providerId });
}
function requireDefaults(value: NodeDefaultsOpening | null | undefined): NodeDefaultsOpening {
  if (value === null || value === undefined) throw new Error('The opted-in native Defaults projections must be present');
  return value;
}

it('keeps ordinary Chat openings read-free for registry, settings, secrets and provider suppliers', async () => {
  const world = new World(), person = reader(world), native = world.requestFeeds.read.bind(world.requestFeeds);
  const spy = vi.spyOn(world.requestFeeds, 'read').mockImplementation(native);
  try {
    const result = await nodeChatOpening(world, person, id, new URLSearchParams());
    expect(result.defaults).toBeUndefined();
    expect(spy.mock.calls.map(([, path]) => path).filter((path) => /\/(registry|settings|secrets)$|^\/providers\//.test(path))).toEqual([]);
  } finally { spy.mockRestore(); }
});

it('supplies exactly the same current-reader Node, registry, settings, secret names and selected provider', async () => {
  const world = new World(), person = reader(world); selected(world, person);
  const query = new URLSearchParams({ defaults: 'true', secret_names: 'true', default_provider_name: 'true' });
  const result = await nodeChatOpening(world, person, id, query), meta = requireDefaults(result.defaults);
  expect(meta.node).toEqual(world.places.node(person, id));
  expect(meta.registry).toEqual((await world.requestFeeds.read(person, '/settings/registry')).body);
  expect(meta.settings).toEqual((await world.requestFeeds.read(person, `/nodes/${id}/settings`)).body);
  expect(meta.secrets).toEqual((await world.requestFeeds.read(person, `/nodes/${id}/secrets`)).body);
  expect(meta.providers).toEqual([{ setting: 'agent.default_provider_id', id: providerId,
    provider: (await world.requestFeeds.read(person, `/providers/${providerId}`)).body, problem: null, retry_after: null }]);
  expect(meta.registry_problem).toBeNull(); expect(meta.settings_problem).toBeNull(); expect(meta.secrets_problem).toBeNull();
  expect(result.opening?.sync.chat.node?.id).toBe(id);
});

it('does not call secret or selected-provider suppliers when their actual demand flags are absent', async () => {
  const world = new World(), person = reader(world); selected(world, person);
  const native = world.requestFeeds.read.bind(world.requestFeeds), spy = vi.spyOn(world.requestFeeds, 'read').mockImplementation(native);
  try {
    const result = await nodeChatOpening(world, person, id, new URLSearchParams({ defaults: 'true' }));
    const meta = requireDefaults(result.defaults);
    expect(meta.secrets).toBeNull(); expect(meta.secrets_problem).toBeNull(); expect(meta.providers).toEqual([]);
    expect(spy.mock.calls.map(([, path]) => path).filter((path) => /\/secrets$|^\/providers\//.test(path))).toEqual([]);
  } finally { spy.mockRestore(); }
});

it('retains a native selected-provider denial and Retry-After without replacing the independent Chat or settings', async () => {
  const world = new World(), person = reader(world); selected(world, person);
  const native = world.requestFeeds.read.bind(world.requestFeeds), refusal = new Refusal(403, 'authz.denied', 'The selected provider is not readable.', {}, { 'Retry-After': '3' });
  const spy = vi.spyOn(world.requestFeeds, 'read').mockImplementation((who, path) => {
    if (path === `/providers/${providerId}`) return Promise.reject(refusal);
    return native(who, path);
  });
  try {
    const result = await nodeChatOpening(world, person, id, new URLSearchParams({ defaults: 'true', default_provider_name: 'true' }));
    const meta = requireDefaults(result.defaults);
    expect(meta.providers).toEqual([{ setting: 'agent.default_provider_id', id: providerId, provider: null, problem: refusal.answer.body, retry_after: 3 }]);
    expect(meta.settings).toEqual((await native(person, `/nodes/${id}/settings`)).body);
    expect(result.opening?.sync.chat.node?.id).toBe(id);
    expect(spy.mock.calls.filter(([, path]) => path === `/providers/${providerId}`)).toHaveLength(1);
  } finally { spy.mockRestore(); }
});

it('serves the existing genuine Host setting used by the Defaults lazy-options journey', async () => {
  const world = new World(), person = reader(world), host = SETTINGS_REGISTRY.find((def) => def.key === 'env.host_id');
  expect(host).toBeDefined();
  const reply = await nodeChatOpening(world, person, ACME.web, new URLSearchParams({ defaults: 'true' }));
  const meta = requireDefaults(reply.defaults);
  expect(meta.registry?.find((def) => def.key === 'env.host_id')).toEqual(host);
  expect(meta.settings?.find((row) => row.key === 'env.host_id')).toMatchObject({ value: null, own: null, writable: true });
});

it('serves the native link-sharing policy to a thread and its joined Chat opening', async () => {
  const world = new World(), person = reader(world);
  const policy = SETTINGS_REGISTRY.find((def) => def.key === 'sharing.on_link');
  expect(policy).toBeDefined();
  const direct = await world.requestFeeds.read(person, `/nodes/${ACME.mainThread}/settings`);
  const rows = direct.body as ResolvedSetting[];
  expect(rows.find((row) => row.key === 'sharing.on_link')).toMatchObject({ value: true, own: null, locked_by: null });
  const opened = requireDefaults((await nodeChatOpening(world, person, ACME.mainThread, new URLSearchParams({ defaults: 'true' }))).defaults);
  expect(opened.registry?.find((def) => def.key === 'sharing.on_link')).toEqual(policy);
  expect(opened.settings?.find((row) => row.key === 'sharing.on_link')).toEqual(rows.find((row) => row.key === 'sharing.on_link'));
});
