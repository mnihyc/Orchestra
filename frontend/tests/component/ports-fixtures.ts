/** Small typed Ports projections for component behavior; no request or socket success is simulated here. */
import type { Environment, ResolvedSetting, PortCapability } from '@/generated/api-types';
import type { EnvironmentPorts, ListeningPort } from '@/features/ports/presentation';

const setting = (key: string, value: unknown): ResolvedSetting => ({ key, value, source: { scope: 'default', node: null }, own: null, locked_by: null, writable: false, parts: [] });
export const portsEnvironment = { id: 'environment-1', name: 'Sandbox', placement: 'managed', host: { id: 'host-1', name: 'host-1' }, network: { mode: 'open' }, access: { permissions: ['ports.view', 'ports.manage', 'browser.manage'] } } as Environment;
/** One audience's capability as the server answers a Ports manager: shipped, place asks and the others are denied. */
export function portCapability(kind: PortCapability['kind'], decision: PortCapability['decision'], granted = false): PortCapability {
  const key = `port_${kind}`;
  return { key, kind, label: kind === 'place' ? 'Ports for a place' : kind === 'link' ? 'Ports through a link' : 'Ports on the host', risk: kind === 'place' ? 'medium' : 'high', offered: true, granted, from_template: false, decision, approver_scope: 'environment', admission: granted ? { capability: key, source: 'grant', scope: null, by: { id: 'user-1', username: 'tester', display_name: 'Tester', kind: 'human', avatar: { text: 'T', color: 0 } } } : null, can_grant: !granted && decision !== 'deny', can_revoke: granted };
}
export const portListener: ListeningPort = { port: 3000, protocol: 'tcp', generation: 1, identity: 'socket-web-1', local_address: '127.0.0.1:3000', first_seen_at: '2026-10-13T09:00:00Z', internal: false, unavailable_reason: null, process: { pid: 501, start: '2026-10-13T09:00:00Z', name: 'web-server', thread: { kind: 'thread', id: 'thread-1', name: 'Repro', redacted: false }, item_id: null, unknown_work: false }, openings: [] };

export function portsProjection(mode = 'full'): EnvironmentPorts {
  const listeners: ListeningPort[] = mode === 'empty' ? [] : [
    { ...portListener },
    { ...portListener, port: 5353, protocol: 'udp', identity: 'socket-udp-1', local_address: '127.0.0.1:5353', process: null },
    { ...portListener, port: 9222, internal: true, unavailable_reason: 'The browser control endpoint is Internal.', process: null },
  ];
  const ports = [3000, 5353, 9222].map((port) => ({ port, url: `https://stable-p${port}.ports.example`, name: `stable-p${port}`, helper_command: `orch-connector port forward --environment environment-1 --port ${port} --token-stdin`, listeners: listeners.filter((listener) => listener.port === port), openings: [], internal: port === 9222, state: mode === 'offline' ? 'stopped' as const : mode === 'unsupported' ? 'unavailable' as const : mode === 'empty' ? 'nothing_listens' as const : 'listening' as const, reason: null, can: { enter: true, let_more_in: true, here: true } }));
  return { snapshot: { environment_id: portsEnvironment.id, generation: 1, as_of: '2026-10-13T09:00:00Z', discovery: mode === 'offline' ? 'offline' : mode === 'unsupported' ? 'unsupported' : 'native', reason: null, listeners: [] }, listeners, ports, openings: [], can: { open_place: true, make_link: true, publish: true }, host_ports: { bind: 'all', address: 'host.example', ranges: [{ from: 41000, to: 41015 }] }, policy: { enabled: setting('ports.enabled', true), max_open: setting('ports.max_open', 16), link_max_secs: setting('ports.link_max_secs', 86_400) }, capabilities: [portCapability('place', 'ask'), portCapability('link', 'deny'), portCapability('host', 'deny')], reasons: [], ports_exposed: false, browser_available: true };
}
