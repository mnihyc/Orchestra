/** Converts served Ports contracts into compact display values without inventing admission or entries. */
import type { ResolvedSetting } from '@/generated/api-types';
import type * as Wire from '@/generated/api-types';
import type * as View from './presentation';

export const transportsFor = (choice: View.HostTransports): Wire.PortTransport[] => choice === 'tcp_udp' ? ['tcp', 'udp'] : [choice];
export const transportChoice = (values: readonly Wire.PortTransport[]): View.HostTransports => values.includes('tcp') && values.includes('udp') ? 'tcp_udp' : values.includes('udp') ? 'udp' : 'tcp';
export const methodsFor = (choice: View.LinkCarriers): ('browser' | 'helper')[] => choice === 'both' ? ['browser', 'helper'] : [choice];
export const methodChoice = (values: readonly Wire.PortEntryMethod[]): View.LinkCarriers => values.includes('browser') && values.includes('helper') ? 'both' : values.includes('browser') ? 'browser' : 'helper';
export const programFor = (http: View.PortHttp): Wire.PortProgram => ({ http_scheme: http.scheme, host_header: http.host_header, proxy_protocol: http.proxy_protocol === true });
export const programView = (program: Wire.PortProgram): View.PortHttp => ({ scheme: program.http_scheme, host_header: program.host_header, proxy_protocol: program.proxy_protocol });

/** Missing settings remain unreported, never shipped values guessed by a screen. */
function setting(key: string, value: unknown, scope: Wire.EnvironmentPorts['allowed'][number]['scope']): ResolvedSetting {
  return { key: `ports.${key}`, value, source: { scope: scope?.kind ?? 'default', node: scope }, own: null, locked_by: null, writable: false, parts: [] };
}
export function openingView(opening: Wire.PortOpening): View.PortOpening {
  const host = opening.entries.find((entry) => entry.method === 'host');
  const browser = opening.entries.find((entry) => entry.method === 'browser');
  const by = opening.allowed_by ?? opening.requested_by;
  return { id: opening.id, environment_id: opening.environment_id, port: opening.port, way: opening.kind, transports: transportChoice(opening.transports), opened_by: by, requested_by: opening.requested_by, requested_by_thread: opening.requested_by_thread, allowed_by: opening.allowed_by, admission: opening.admission ?? null, label: opening.label, opened_at: opening.opened_at, until: opening.until, desired: !['waiting','closed','refused'].includes(opening.state), state: opening.state === 'not_listening' ? 'nothing_listens' : opening.state, reason: opening.reason, name: browser?.address ?? null, url: opening.kind === 'host' ? host?.address ?? null : browser?.address ?? null, host: opening.host, publisher: opening.publisher, host_port: opening.host_port, fixed_host_port: opening.fixed_host_port, host_allocated: opening.host_allocated, revision: opening.revision, place: opening.place, viewers: null, link_carriers: opening.kind === 'link' ? methodChoice(opening.entry_methods) : null, http: programView(opening.program), connection_count: opening.counts?.active ?? null, entries: opening.entries, can: { enter: opening.can.use, close: opening.can.close, change: opening.can.change } };
}
export function targetView(target: Wire.PortTarget): View.EnvironmentPort {
  const helper = target.own_entries.find((entry) => entry.method === 'helper');
  return { port: target.port, counts: target.counts?.active ?? null, name: target.address, url: target.address, helper_command: helper?.command ?? '', entries: target.own_entries, as_of: target.as_of, discovery: target.discovery, listeners: target.listeners.map((listener) => ({ port: listener.port, protocol: listener.transport, binding: listener.binding, local_address: listener.local_address, generation: listener.generation, identity: listener.observation_id, first_seen_at: listener.first_seen_at, internal: listener.internal, unavailable_reason: listener.unavailable_reason, process: listener.process === null ? null : { pid: listener.process.pid, start: listener.process.start, name: listener.process.name, thread: null, item_id: listener.process.item_id, unknown_work: listener.process.thread_id === null }, openings: [] })), openings: target.openings.map(openingView), internal: target.listeners.some((listener) => listener.internal), state: target.state === 'observed' ? 'listening' : target.state === 'not_listening' ? 'nothing_listens' : target.state, reason: target.reason, program: programView(target.program), revision: target.revision, can: { enter: target.own_entries.length > 0, let_more_in: true, here: true } };
}
export function portsView(data: Wire.EnvironmentPorts, settings: readonly ResolvedSetting[] = []): View.EnvironmentPorts {
  const choice = (kind: Wire.PortOpening['kind']) => data.allowed.find((entry) => entry.kind === kind);
  const resolved = (key: string, fallback: unknown, kind: Wire.PortOpening['kind']) => settings.find((entry) => entry.key === `ports.${key}`) ?? setting(key, fallback, choice(kind)?.scope ?? null);
  const targets = data.targets.map(targetView);
  for (const target of targets) target.can.let_more_in = target.openings.some((opening) => opening.desired) || data.used < data.limit;
  return { snapshot: { environment_id: data.environment_id, generation: data.snapshot.generation, as_of: data.snapshot.as_of, discovery: data.snapshot.discovery, reason: data.snapshot.reason, listeners: [], udp_dropped: data.snapshot.udp_dropped }, listeners: targets.flatMap((target) => target.listeners), ports: targets, openings: data.openings.map(openingView), policy: { enabled: resolved('enabled', null, 'place'), max_open: resolved('max_open', data.limit, 'place'), link_max_secs: resolved('link_max_secs', null, 'link') }, capabilities: data.capabilities, host_ports: data.host_ports === null ? null : { bind: data.host_ports.listen, address: data.host_ports.incoming_address, ranges: data.host_ports.ranges }, ports_exposed: data.ports_exposed, used: data.used, truncated: data.truncated, can: { open_place: choice('place')?.allowed === true, make_link: choice('link')?.allowed === true, publish: choice('host')?.allowed === true }, reasons: data.allowed.filter((entry) => !entry.allowed || entry.reason !== null).map((entry) => ({ act: entry.kind, code: 'ports.not_allowed', message: entry.reason ?? '', scope: entry.scope, holders: entry.holders })), browser_available: data.targets.some((target) => target.address !== null) };
}
export function planView(plan: Wire.PortOpeningPlan, incomingAddress: string | null = null): View.PortPublishPlan {
  const entry = plan.target.own_entries.find((item) => item.method === 'browser');
  const host = plan.candidate?.host ?? null;
  const address = plan.candidate === null ? entry?.address ?? '' : incomingAddress === null ? '' : `${incomingAddress.includes(':') && !incomingAddress.startsWith('[') ? `[${incomingAddress}]` : incomingAddress}:${plan.candidate.host_port}`;
  return { preflight_id: plan.preflight_id, expires_at: plan.expires_at, environment_id: plan.target.environment_id, port: plan.target.port, transports: transportChoice(plan.request.transports ?? ['tcp', 'udp']), host, publisher: plan.candidate?.machine ?? null, host_ports_revision: plan.candidate?.host_revision ?? 0, host_port: plan.candidate?.host_port ?? 0, fixed: plan.candidate?.fixed === true, address, audience: plan.audience.sentence, blockers: [...plan.blockers, ...(plan.candidate !== null && incomingAddress === null ? [{ code: 'ports.unavailable', message: 'The current host declaration must be reviewed.' }] : [])] };
}
export function connectionsView(data: Wire.PortConnectionList): View.PortConnections {
  return { count: data.counts.active, limit: 128, as_of: data.as_of, connections: (data.connections ?? []).map((connection) => ({ id: connection.id, opening_id: connection.opening_id ?? '', caller: connection.outside.caller.kind === 'person' ? { kind: 'user', user: connection.outside.caller.user } : connection.outside.caller.kind === 'link' ? { kind: 'link', label: connection.outside.caller.label } : { kind: 'outside' }, outside: { address: connection.outside.address, port: connection.outside.port }, inside: connection.inside, since: connection.since, last_active_at: connection.last_active, bytes_to_service: connection.bytes_in, bytes_from_service: connection.bytes_out, requests: connection.http_requests, state: connection.ended_at === null ? 'active' : 'closed' })) };
}
