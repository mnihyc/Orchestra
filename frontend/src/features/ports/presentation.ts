/** Compact Ports presentation values adapted from the served wire facts, without authority inferred from names. */
import type { PortApproval } from './pending';
import type { Environment, Holder, HostRef, MachineRef, NodeRef, ResolvedSetting, UserRef, PortCapability, PortEntry, PortOpeningAdmission, PortTarget, PortSnapshot as ServedPortSnapshot, HostPortsLimits } from '@/generated/api-types';

export type PortProtocol = 'tcp' | 'udp';

export type HostTransports = 'tcp_udp' | 'tcp' | 'udp';

export type LinkCarriers = 'browser' | 'helper' | 'both';

export interface HostPorts { bind: string; address: string; ranges: { from: number; to: number }[] }

export interface PortHttp { scheme: 'http' | 'https'; host_header: string | null; proxy_protocol?: boolean }

export interface PortOpening {
  id: string; environment_id: string; port: number; way: 'place' | 'link' | 'host';
  transports: HostTransports; opened_by: UserRef | null; entries?: PortEntry[]; opened_at: string; until: string | null;
  requested_by?: UserRef | null; requested_by_thread?: NodeRef | null; allowed_by?: UserRef | null; admission?: PortOpeningAdmission | null; label?: string; link_recipient?: 'thread' | 'person';
  desired: boolean; state: 'waiting' | 'opening' | 'open' | 'nothing_listens' | 'stopped' | 'unavailable' | 'refused' | 'failed' | 'closed' | 'expired';
  reason: string | null; name: string | null; url: string | null; host: HostRef | null; publisher?: MachineRef | null;
  host_port: number | null; fixed_host_port: boolean; revision: number;
  place: NodeRef | null; viewers: number | null; link_carriers: LinkCarriers | null;
  http: PortHttp; host_allocated?: boolean; connection_count: number | null;
  can: { enter: boolean; close: boolean; change: boolean };
}

export interface ObservedPort {
  port: number; protocol: PortProtocol; local_address: string; generation: number; identity: string;
  first_seen_at: string; internal: boolean; unavailable_reason: string | null;
  process: { pid: number; start: string; name: string; thread_id: string | null; item_id: string | null } | null;
}

export interface ListeningPort extends Omit<ObservedPort, 'process'> {
  binding?: 'listening' | 'bound_unknown';
  process: { pid: number; start: string; name: string; thread: NodeRef | null; item_id: string | null; unknown_work: boolean } | null;
  openings: PortOpening[];
}

export interface EnvironmentPort {
  port: number; name: string | null; url: string | null; helper_command: string; program?: PortHttp; revision?: number;
  counts?: number | null; entries?: PortEntry[]; as_of?: string; discovery?: PortTarget['discovery'];
  listeners: ListeningPort[]; openings: PortOpening[]; internal: boolean;
  state: 'listening' | 'nothing_listens' | 'stopped' | 'unavailable'; reason: string | null;
  can: { enter: boolean; let_more_in: boolean; here: boolean };
}

export interface PortSnapshot { environment_id: string; generation: number; as_of: string; discovery: 'native' | 'unsupported' | 'offline'; reason: string | null; listeners: ObservedPort[]; udp_dropped?: ServedPortSnapshot['udp_dropped'] }
export interface PortReason { act: string; code: string; message: string; scope: NodeRef | null; holders: Holder[] }
export interface EnvironmentPorts {
  snapshot: PortSnapshot; listeners: ListeningPort[]; ports: EnvironmentPort[]; openings: PortOpening[];
  policy: { enabled: ResolvedSetting; max_open: ResolvedSetting; link_max_secs: ResolvedSetting };
  /** Each audience's capability here: granted, from the template or by the policy, and what this reader may change. */
  capabilities: PortCapability[];
  host_ports: HostPorts | null; ports_exposed: boolean;
  can: { open_place: boolean; make_link: boolean; publish: boolean }; used?: number; truncated?: boolean;
  reasons: PortReason[]; browser_available: boolean;
}

export interface PortPlacePreview { place: NodeRef; viewers: number | null; allowed: boolean; reason: string | null; preflight_id: string }

export interface PortPublishPlan {
  preflight_id: string; expires_at: string; environment_id: string; port: number;
  transports: HostTransports; host: HostRef | null; publisher?: MachineRef | null; host_ports_revision: number; host_port: number;
  fixed: boolean; address: string; audience: string; blockers: { code: string; message: string }[];
}

export interface PortLinkReply { opening: PortOpening; link_url_once: string | null; browser_url_once?: string | null; helper_link_once?: string | null }

export interface PortConnection {
  id: string; opening_id: string;
  caller: { kind: 'user'; user: UserRef } | { kind: 'link'; label: string } | { kind: 'outside' };
  outside: { address: string; port: number }; inside: { address: string; port: number } | null;
  since: string; last_active_at: string; bytes_to_service: number; bytes_from_service: number;
  requests: number | null; state: 'active' | 'closed';
}
export interface PortConnections { count: number; limit: number; as_of: string; connections: PortConnection[] }

export type PortsEnvironment = Environment & { ports_exposed?: boolean };
export function isExposed(environment: { ports_exposed?: boolean }): boolean { return environment.ports_exposed === true; }

export function exposureOf(environment: { id: string }): { exposed: boolean; mayView: boolean } {
  const row = environment as { ports_exposed?: boolean; access?: { permissions: string[] } };
  return { exposed: row.ports_exposed === true, mayView: row.access?.permissions.includes('ports.view') === true };
}

export interface PortsBrowser { reason: string | null; open: (port: EnvironmentPort) => void }

export interface HostPortAllocation { opening_id: string; environment: NodeRef; port: number; transports: HostTransports; state: PortOpening['state'] }
export interface HostPortsReview { sources?: { incoming_address: string; listen: string; ranges: string }; as_of?: string; limits?: HostPortsLimits; current: HostPorts | null; proposal: HostPorts; allocations: HostPortAllocation[] | null; revision: number; acknowledged: boolean | null; can_edit: boolean; reason: string | null }

export interface PortRequest { id: string; revision: number; environment_id: string; port: number; way: PortOpening['way']; reason: string; agent: string; address: string | null; audience: string; until: string | null; can_decide: boolean; state: 'pending' | 'allowed' | 'denied' | 'expired' | 'cancelled'; transports: HostTransports; link_carriers: LinkCarriers | null; approval?: PortApproval }
export interface PortRequestResult { request: PortRequest; opening: PortOpening | null; link_url_once: string | null; browser_url_once?: string | null; helper_link_once?: string | null; delivered_to: 'human' | 'thread' | null }
