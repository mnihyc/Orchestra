/** Named screen states use the same stored rows and authorized readers as the fake HTTP API. */
import { backgroundRoster, backgroundWake, backgroundWork, wakeCause } from './background-wake.ts';
import { ACME, id, RUNS_ON, userRef } from './fixtures.ts';
import { catalogue, connector, tunnel } from './gateways.ts';
import { agentMessage, command, marker, threadInput, toolCall, turn } from './thread-rows.ts';
import { Refusal, type World } from './world.ts';

export const SCREEN_FIXTURES = ['background-running', 'background-finished', 'background-off', 'background-lost', 'background-held',
  'gateways-catalogue', 'gateways-edit', 'gateway-automatic', 'gateway-offline', 'gateway-unreported', 'tunnel-attached', 'tunnel-unavailable',
  'agent-share-ladders', 'agent-access-ladders', 'thread-options-allowed', 'thread-options-fixed', 'provider-search-models', 'provider-search-editor'] as const;
export type ScreenFixture = (typeof SCREEN_FIXTURES)[number];

export function installScreenFixture(world: World, name: unknown): void {
  if (!SCREEN_FIXTURES.includes(name as ScreenFixture)) throw new Refusal(400, 'request.invalid', 'Unknown screen fixture.');
  if (typeof name === 'string' && (name.startsWith('gateway') || name.startsWith('tunnel-'))) {
    world.admin.gatewayCatalogue = structuredClone(catalogue);
    let status = { ...structuredClone(connector), machine_id: RUNS_ON.laptop };
    if (name === 'gateway-offline') status = { ...status, selection: 'selected', selected_gateway_id: 'removed', primary_gateway_id: null, connected: false };
    if (name === 'gateway-unreported') status = { ...status, selection: null, primary_gateway_id: null, measured_at: null, measurements: [] };
    world.resources.gatewayStatuses.set(RUNS_ON.laptop, status);
    let carrier = { ...structuredClone(tunnel), thread_id: ACME.mainThread };
    if (name === 'tunnel-unavailable') carrier = { ...carrier, state: 'unavailable', attached_gateway_id: null,
      refusal: { code: 'tunnel.gateway_unreachable', detail: 'Europe gateway: source could not attach; Asia tools: peer could not attach.', attempted_gateways: ['eu', 'asia'] } };
    world.threads.gatewayStatuses.set(ACME.mainThread, carrier);
    return;
  }
  const person = world.people.get('ada');
  if (person === undefined) throw new Error('The screen fixture needs its ordinary thread owner.');
  if (name === 'agent-share-ladders' || name === 'agent-access-ladders') {
    const response = world.access.shares.handle(person, 'POST', '/share/agents', new URLSearchParams(), {
      client_id: 'screen-agent-management', what: { node_id: ACME.mainThread }, to: [{ node_id: ACME.web }], mode: 'live',
      parts: { timeline: 'manage', chat: 'manage', documents: 'manage', thread_fork: true, thread_archive: true }, download: false,
    });
    if (response?.status !== 201) throw new Error('The management ladder fixture needs its recorded ordinary-author contribution.');
    return;
  }
  if (typeof name === 'string' && (name.startsWith('thread-options-') || name.startsWith('provider-search-'))) {
    const binding = world.resources.bindingOf(ACME.mainThread);
    const agent = world.resources.agents.find((row) => row.id === binding.agent);
    if (agent === undefined) throw new Error('The native options fixture needs its declared agent.');
    const parameters = world.resources.declaredBy(agent.id).filter((def) => def.key !== 'web_search');
    agent.descriptor = { ...agent.descriptor, parameters: [...parameters, {
      key: 'web_search', title: 'Web search', description: 'Native public web search.', type: 'enum', group: 'behaviour', apply: 'new_session',
      scopes: ['org', 'project', 'topic', 'thread'], options: { from_model: 'web_search' }, default: { from_model: 'default_web_search' },
      eligible_when: { model_has: 'web_search' },
    }] };
    world.threads.setEntry('screen-model', 'Screen model', { context_window: 200_000, efforts: ['low', 'medium'], default_effort: 'medium',
      offered: { web_search: ['cached', 'live'] }, web_search: ['cached', 'live'], default_web_search: 'live',
      rate: { input_per_mtok: '2', output_per_mtok: '8', web_search_per_call: '0.01' },
      request_options: {
        native: { type: 'object', additionalProperties: false, properties: { verbosity: name === 'thread-options-fixed'
          ? { const: 'normal' } : { enum: ['terse', 'normal'], default: 'normal' } } },
        'openai-responses': { type: 'object', additionalProperties: false, properties: { service_tier: { enum: ['standard', 'fast'], default: 'standard' } } },
      },
    }, [], RUNS_ON.openAiKey);
    world.resources.bind(ACME.mainThread, { ...binding, provider: RUNS_ON.openAiKey, model: 'screen-model' });
    world.threads.configure(ACME.mainThread, { reset: true });
    world.threads.emit(ACME.mainThread, { thread: { state: 'idle', model: 'screen-model', model_display: 'Screen model', effort: 'medium' } });
    return;
  }
  const thread = ACME.mainThread, origin = id(0x84, 1), inputId = id(0x84, 2);
  const commandId = id(0x84, 3), childId = id(0x84, 4);
  const lost = name === 'background-lost', finished = name === 'background-finished', held = name === 'background-held';
  const buildLabel = 'Build web and check the authentication routes', reviewLabel = 'Authentication and session reviewer';
  const tasks = finished || held ? [] : [
    backgroundWork({ id: 'build-run', item_id: commandId, turn_id: origin, input_id: inputId, author: userRef(person.me),
      label: buildLabel, state: lost ? 'lost' : 'running', can_end: !lost, lost_at: lost ? '2026-11-06T10:00:00Z' : null }),
    backgroundWork({ id: 'review-run', item_id: childId, turn_id: origin, input_id: inputId, author: userRef(person.me), kind: 'subagent',
      label: reviewLabel, state: lost ? 'lost' : 'running', can_end: false, lost_at: lost ? '2026-11-06T10:00:00Z' : null }),
  ];
  const wake = backgroundWake({ tasks, setting: name !== 'background-off', effective: name !== 'background-off', held_completions: held ? 2 : 0 });
  world.threads.configure(thread, { reset: true });
  const items = finished || held || lost ? [] : [{ id: 'native-build-review', item_id: commandId, turn_id: origin, segment_id: id(0x84, 9),
    command: buildLabel, command_truncated: false, cwd: null, cwd_redacted: false, place: 'environment' as const, since: null, cpu_percent: null, memory_bytes: null }];
  world.threads.emit(thread, { thread: { state: 'idle', background: wake }, background: backgroundRoster({ wake, can_end: !lost, total: items.length, items }),
    turns: [turn(origin, { ordinal: 1, state: 'completed', inputId, completedAt: '2026-11-06T10:00:00Z' })],
    items: [command(commandId, { turn: origin, command: buildLabel, output: '', exit: finished || held ? 0 : null, background: true }),
      toolCall(childId, { kind: 'subagent', name: reviewLabel, child_stream: 'sub:auth-review', role: 'review', prompt: 'Review authentication.', state: finished || held ? 'failed' : 'running' }, { turn: origin, status: finished || held ? 'completed' : 'in_progress' })] });
  if (finished || held) {
    const wakeInput = id(0x84, 5), wakeTurn = id(0x84, 6);
    world.threads.emit(thread, {
      inputs: [{ ...threadInput(wakeInput, 'Continue after background work.', { position: '1', source: 'system', author: userRef(person.me) }),
        state: held ? 'pending' : 'delivered', state_detail: held ? 'background_stopped' : null, turn_id: held ? null : wakeTurn }],
      turns: held ? [] : [turn(wakeTurn, { ordinal: 2, state: 'completed', inputId: wakeInput, completedAt: '2026-11-06T10:01:00Z' })],
      items: [marker(id(0x84, 7), { marker: 'background_wake', data: { admitted: !held, input_id: wakeInput,
        causes: [wakeCause({ label: buildLabel, origin_turn: origin, native_item_ref: commandId }), wakeCause({ id: 'auth-review', kind: 'subagent', label: reviewLabel, state: 'failed', exit_code: null, origin_turn: origin, native_item_ref: childId })] } }, { turn: held ? null : wakeTurn }),
        ...(held ? [] : [agentMessage(id(0x84, 8), 'The background work is ready to review.', { turn: wakeTurn })])],
    });
  }
}
