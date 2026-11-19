/** The Ports double proves the screen contract's authority, pure reads, reviewed effects and safe delivery fences. */
import { describe, expect, it } from 'vitest';
import type { PortApproval, PortApprovalReply } from '@/features/ports/pending';
import { requestView } from '@/api/ports';
import { openingView } from '@/features/ports/wire';
import { ACME, RUNS_ON } from '../doubles/fixtures';
import { Refusal, World } from '../doubles/world';
import type { PortOpenReply, PortOpeningPlan, PortTarget } from '@/generated/api-types';

function fixture() {
  const world = new World(); const person = world.people.get('ada')!;
  world.ports.control('/set', { scene: 'full' });
  const path = `/environments/${RUNS_ON.sandbox}/ports`;
  const call = (method: string, route: string, body: Record<string, unknown> = {}, query = '') => world.ports.handle(person, method, route, new URLSearchParams(query), body);
  return { world, person, path, call };
}
describe('the current Ports producer', () => {
  it('reads an unlisted stable target at revision zero without inserting a listener or opening; its first write is fenced', async () => {
    const { call, path, world } = fixture();
    const target = (await call('GET', `${path}/8080`))?.body as PortTarget;
    expect(target).toMatchObject({ port: 8080, revision: 0, state: 'not_listening', discovery: 'native', listeners: [], openings: [] });
    expect(target.address).toContain('-p8080.ports.example');
    const options = { http_scheme: 'https', host_header: 'localhost:8080', proxy_protocol: true };
    expect((await call('PATCH', `${path}/8080/program`, { program: options, expected_revision: 0 }))?.body).toMatchObject({ revision: 1, program: options });
    await expect(call('PATCH', `${path}/8080/program`, { program: options, expected_revision: 0 })).rejects.toBeInstanceOf(Refusal);
    expect(world.ports.control('/state', {})?.body).toMatchObject({ openings: [] });
  });
  it('requires one stored link review and reconciles a lost person-once reply without a duplicate or retrievable proof', async () => {
    const { call, path, world } = fixture(); world.ports.control('/set', { scene: 'full', lost_link: true });
    const plan = (await call('POST', `${path}/3000/openings/preflight`, { port: 3000, kind: 'link', label: 'Webhook', reason: '', until: new Date(Date.now() + 3_600_000).toISOString(), entry_methods: ['browser','helper'], transports: ['tcp','udp'] }))?.body as PortOpeningPlan;
    expect(world.ports.control('/state', {})?.body).toMatchObject({ openings: [] });
    const body = { client_id: 'one', preflight_id: plan.preflight_id };
    await expect(call('POST', `${path}/3000/openings`, body)).rejects.toBeInstanceOf(Refusal);
    const replay = (await call('POST', `${path}/3000/openings`, body))?.body as PortOpenReply;
    expect(replay.link?.url_once).toBeNull(); expect(replay.opening.label).toBe('Webhook');
    expect((world.ports.control('/state', {})?.body as { openings: unknown[] }).openings).toHaveLength(1);
    expect(JSON.stringify((await call('GET', path))?.body)).not.toMatch(/orch_token|#[a-f\d-]{36}/);
    await expect(call('POST', `${path}/3000/openings`, { ...body, preflight_id: 'different' })).rejects.toBeInstanceOf(Refusal);
  });
  it('keeps host allocation separate from not-listening and refuses a taken candidate', async () => {
    const { call, path, world } = fixture(); world.ports.control('/set', { scene: 'empty' });
    const request = { port: 3000, kind: 'host', reason: '', transports: ['tcp','udp'] };
    const plan = (await call('POST', `${path}/3000/openings/preflight`, request))?.body as PortOpeningPlan;
    world.ports.control('/race', {});
    await expect(call('POST', `${path}/3000/openings`, { client_id: 'taken', preflight_id: plan.preflight_id })).rejects.toBeInstanceOf(Refusal);
    const reviewed = (await call('POST', `${path}/3000/openings/preflight`, request))?.body as PortOpeningPlan;
    const reply = (await call('POST', `${path}/3000/openings`, { client_id: 'fresh', preflight_id: reviewed.preflight_id }))?.body as PortOpenReply;
    expect(reply.opening).toMatchObject({ state: 'not_listening', host_allocated: true, host_port: 41000 });
    expect(reply.opening.entries[0]?.address).toBe('host.example:41000');
  });
  it('uses one ordinary port approval, coalesces reason-only retries, and refuses a choice that is no longer one', async () => {
    const { call, world } = fixture(); const request = { port: 3000, kind: 'link', reason: 'Webhook', until: new Date(Date.now() + 3_600_000).toISOString() };
    world.places.denied.set('ada', new Set(['thread.manage']));
    const first = (await call('POST', `/threads/${ACME.repro}/port-requests`, request))?.body as PortOpenReply;
    const repeated = (await call('POST', `/threads/${ACME.repro}/port-requests`, { ...request, reason: 'A different explanation' }))?.body as PortOpenReply;
    expect(repeated.request?.id).toBe(first.request?.id);
    await expect(call('POST', `/approvals/${first.request?.id}/decision`, { decision: 'always', expected_revision: 1 })).rejects.toBeInstanceOf(Refusal);
    const before = world.threads.snapshot(ACME.repro) as { turns: unknown[]; inputs: unknown[] };
    const decision = (await call('POST', `/approvals/${first.request?.id}/decision`, { decision: 'once', expected_revision: 1, link_recipient: 'person' }))?.body as PortApprovalReply;
    expect(decision.opening_result?.link?.recipient).toBe('person'); expect(decision.opening_result?.link?.browser_url_once).toContain('orch_token=');
    const stored = (await call('GET', `/approvals/${first.request?.id}`))?.body as PortApproval;
    expect(stored.state).toBe('approved'); expect(JSON.stringify(stored)).not.toContain('orch_token=');
    expect(requestView(stored).state).toBe('allowed');
    expect(() => requestView({ ...stored, state: 'answered' })).toThrow('a port permission cannot be answered with content');
    const after = world.threads.snapshot(ACME.repro) as { turns: unknown[]; inputs: unknown[]; items: { kind: string; payload: unknown }[] };
    expect(after.turns).toHaveLength(before.turns.length); expect(after.inputs).toHaveLength(before.inputs.length);
    expect(after.items.at(-1)).toMatchObject({ kind: 'user_message', payload: { source: 'system' } });
    expect(JSON.stringify(after.items.at(-1))).not.toMatch(/orch_token=|<orch/);
  });
  it('grants a card to the environment, opens the next request at once under that grant, and asks again once it is taken back', async () => {
    const { call } = fixture(); const request = { port: 3000, kind: 'place', place_id: ACME.web, reason: 'Preview' };
    const first = (await call('POST', `/threads/${ACME.repro}/port-requests`, request))?.body as PortOpenReply;
    const decided = (await call('POST', `/approvals/${first.request?.id}/decision`, { decision: 'grant', expected_revision: 1 }))?.body as PortApprovalReply;
    expect(decided.opening_result?.opening.admission).toMatchObject({ capability: 'port_place', source: 'grant' });
    const next = (await call('POST', `/threads/${ACME.repro}/port-requests`, { ...request, port: 3001 }))?.body as PortOpenReply;
    expect(next.request).toBeNull(); expect(next.opening.state).not.toBe('waiting'); expect(next.opening.admission?.source).toBe('grant');
    await call('DELETE', `/environments/${RUNS_ON.workspace}/capabilities/port_place`);
    const asked = (await call('POST', `/threads/${ACME.repro}/port-requests`, { ...request, port: 3002 }))?.body as PortOpenReply;
    expect(asked.opening.state).toBe('waiting');
  });
  it('keeps unavailable, waiting, refused and unknown counts distinct in the screen adapter', async () => {
    const { call, path } = fixture();
    const plan = (await call('POST', `${path}/3000/openings/preflight`, { port: 3000, kind: 'place', place_id: ACME.web, reason: '' }))?.body as PortOpeningPlan;
    const reply = (await call('POST', `${path}/3000/openings`, { client_id: 'place', preflight_id: plan.preflight_id }))?.body as PortOpenReply;
    for (const state of ['waiting','refused','unavailable'] as const) expect(openingView({ ...reply.opening, state, counts: null }).state).toBe(state);
    expect(openingView({ ...reply.opening, counts: null }).connection_count).toBeNull();
  });
  it('lists effective stopped admissions below a place and closes through the current opening revision', async () => {
    const { call, path, world } = fixture();
    const plan = (await call('POST', `${path}/3000/openings/preflight`, { port: 3000, kind: 'place', place_id: ACME.web, reason: '' }))?.body as PortOpeningPlan;
    await call('POST', `${path}/3000/openings`, { client_id: 'outside', preflight_id: plan.preflight_id });
    world.ports.control('/lifecycle', { outcome: 'stop' });
    const page = (await call('GET', `/nodes/${ACME.org}/port-openings`))?.body as { items: PortOpenReply['opening'][] };
    expect(page.items).toHaveLength(1); expect(page.items[0]).toMatchObject({ kind: 'place', state: 'stopped', can: { close: true } });
    const opening = page.items[0]!;
    expect((await call('DELETE', `/port-openings/${opening.id}`, {}, `expected_revision=${opening.revision}`))?.body).toMatchObject({ state: 'closed' });
    expect(((await call('GET', `/nodes/${ACME.org}/port-openings`))?.body as { items: unknown[] }).items).toHaveLength(0);
  });
});

it('projects own-port callers only to current managers, without a self opening', async () => {
 const world = new World(); const person = world.people.get('ada')!; world.ports.control('/set', { scene: 'full' }); world.ports.control('/own-connections', {});
 world.places.denied.set('ada', new Set(['ports.manage']));
 const reply = await world.ports.handle(person, 'GET', `/environments/${RUNS_ON.sandbox}/ports/connections`, new URLSearchParams('port=3000'), {});
 expect(reply?.body).toMatchObject({ counts: { active: 2 }, connections: null, totals: null });
 expect(world.ports.control('/state', {})?.body).toMatchObject({ openings: [] });
});
