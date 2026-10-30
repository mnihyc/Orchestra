/** The Ports door requests: current snapshots, guarded openings, stored publication plans and entry codes. */
import { queryOptions } from '@tanstack/react-query';
import { ApiError, api, ok } from '@/api/client';
import { queryClient } from '@/api/query';
import { environmentSettingsQuery } from '@/api/environments';
import { hostQuery } from '@/api/resources';
import { liveValue } from '@/api/liveValue';
import type { Approval, Host, PortCapability, PortConnectionList, PortDecisionChoice, PortLinkRecipient, PortOpenReply, PortOpeningPage, PortOpeningPlan, PortOpeningRequest, PortProgram, PortTarget, ResolvedSetting } from '@/generated/api-types';
import type { PortApproval } from '@/features/ports/pending';
import type * as View from '@/features/ports/presentation';
import { connectionsView, methodChoice, openingView, planView, portsView, programFor, targetView, transportChoice, transportsFor } from '@/features/ports/wire';

export const portsKey = (id: string) => ['environment-ports', id] as const;
const withSignal = (signal?: AbortSignal) => (signal === undefined ? {} : { signal });
const portPath = (id: string, port: number) => ({ path: { id, port: String(port) } });

/** A port approval, from an approval the API answered; any other kind is not one. */
function portApproval(approval: Approval): PortApproval {
 if (approval.kind !== 'port') throw new Error('the approval is not a port request');
 return approval;
}

async function settingsFor(id: string): Promise<ResolvedSetting[]> {
 try { return await queryClient.query(environmentSettingsQuery(id)); }
 catch (error) { if (error instanceof ApiError && (error.status === 403 || error.status === 404)) return []; throw error; }
}
async function readPorts(id: string, look: boolean, signal?: AbortSignal): Promise<View.EnvironmentPorts> {
 const [data, settings] = await Promise.all([ok(api.GET('/environments/{id}/ports', { params: { path: { id }, ...(look ? { query: { look: true } } : {}) }, ...withSignal(signal) })), settingsFor(id)]);
 const view = portsView(data, settings);
 const ids = [...new Set(data.targets.flatMap((target) => target.listeners.flatMap((listener) => listener.process?.thread_id === null || listener.process?.thread_id === undefined ? [] : [listener.process.thread_id])))];
 const names = ids.map((nodeId) => queryClient.getQueryData<{ id: string; kind: 'thread'; name: string }>(['node', nodeId]));
 for (const port of view.ports) for (const listener of port.listeners) {
  const observation = data.targets.find((target) => target.port === port.port)?.listeners.find((item) => item.observation_id === listener.identity);
  const index = ids.indexOf(observation?.process?.thread_id ?? ''); const found = names[index];
  if (listener.process !== null && found !== undefined) listener.process.thread = { id: found.id, kind: found.kind, name: found.name, redacted: false };
 }
 return view;
}
export const portsQuery = (id: string) => queryOptions({ queryKey: portsKey(id), retry: false, queryFn: ({ signal }): Promise<View.EnvironmentPorts> => liveValue(portsKey(id), signal) });
export function refreshPorts(id: string): Promise<View.EnvironmentPorts> { return readPorts(id, true); }
export async function readPort(id: string, port: number): Promise<View.EnvironmentPort> { return targetView(await ok(api.GET('/environments/{id}/ports/{port}', { params: portPath(id, port) }))); }

/** Every audience shares this stored preflight; no process identity or generation is submitted. */
export async function preflightOpening(id: string, request: PortOpeningRequest): Promise<PortOpeningPlan> {
 return ok(api.POST('/environments/{id}/ports/{port}/openings/preflight', { params: portPath(id, request.port), body: request }));
}
export async function previewPlace(id: string, port: number, place_id: string, http: View.PortHttp = { scheme: 'http', host_header: null }): Promise<View.PortPlacePreview> {
 const plan = await preflightOpening(id, { port, kind: 'place', place_id, transports: ['tcp','udp'], program: programFor(http), reason: '' });
 return { place: plan.audience.place ?? { id: place_id, kind: 'project', name: null, redacted: true }, viewers: plan.audience.count, allowed: plan.blockers.length === 0, reason: plan.blockers[0]?.message ?? null, preflight_id: plan.preflight_id };
}
export async function createOpening(id: string, port: number, client_id: string, preflight_id: string): Promise<PortOpenReply> { return ok(api.POST('/environments/{id}/ports/{port}/openings', { params: portPath(id, port), body: { client_id, preflight_id } })); }
export async function openForPlace(id: string, port: number, _place_id: string, client_id: string, _http: View.PortHttp, preflight_id: string): Promise<View.PortOpening> { return openingView((await createOpening(id, port, client_id, preflight_id)).opening); }
export async function makePortLink(id: string, port: number, client_id: string, _until: string, _carriers: View.LinkCarriers, _transports: View.HostTransports, _http: View.PortHttp, preflight_id: string): Promise<View.PortLinkReply> {
 const reply = await createOpening(id, port, client_id, preflight_id);
 return { opening: openingView(reply.opening), link_url_once: reply.link?.url_once ?? null, browser_url_once: reply.link?.browser_url_once ?? null, helper_link_once: reply.link?.helper_link_once ?? null };
}
export async function previewPublication(id: string, port: number, transports: View.HostTransports, host_port?: number, http: View.PortHttp = { scheme: 'http', host_header: null }): Promise<View.PortPublishPlan> {
 const plan = await preflightOpening(id, { port, kind: 'host', transports: transportsFor(transports), program: programFor(http), ...(host_port === undefined ? {} : { host_port }), reason: '' });
 if (plan.candidate === null) return planView(plan);
 if (plan.candidate.host === null) {
  const ports = await ok(api.GET('/environments/{id}/ports', { params: { path: { id } } }));
  return planView(plan, ports.host_ports?.incoming_address ?? null);
 }
 const host: Host = await ok(api.GET('/hosts/{id}', { params: { path: { id: plan.candidate.host.id } } }));
 const view = planView(plan, host.ports?.incoming_address ?? null);
 if (host.revision !== plan.candidate.host_revision) view.blockers.push({ code: 'conflict.revision', message: 'The host declaration changed. Review again.' });
 return view;
}
export async function publishPort(id: string, port: number, client_id: string, preflight_id: string, _http: View.PortHttp): Promise<View.PortOpening> { return openingView((await createOpening(id, port, client_id, preflight_id)).opening); }
/** The opening's revision goes in the body, as the contract has it. */
export async function closePort(opening: View.PortOpening): Promise<View.PortOpening> { return openingView(await ok(api.DELETE('/port-openings/{opening_id}', { params: { path: { opening_id: opening.id } }, body: { expected_revision: opening.revision } }))); }
export function enterPort(id: string, port: number): Promise<{ url: string; expires_at: string }> { return ok(api.POST('/environments/{id}/ports/{port}/enter', { params: portPath(id, port), body: {} })); }
export function updatePortProgram(id: string, port: number, program: PortProgram, expected_revision: number): Promise<PortTarget> { return ok(api.PATCH('/environments/{id}/ports/{port}/program', { params: portPath(id, port), body: { program, expected_revision } })); }
export const portConnectionsQuery = (environmentId: string, openingId: string, insidePort?: number) => queryOptions({ queryKey: ['port-connections', environmentId, openingId, insidePort] as const, retry: false, queryFn: async ({ signal }): Promise<View.PortConnections> => connectionsView(await ok(api.GET('/environments/{id}/ports/connections', { params: { path: { id: environmentId }, query: { opening_id: openingId, ...(insidePort === undefined ? {} : { inside_port: insidePort }) } }, signal }))) });
/** The same live reader narrows either to an opening or a port, including own uses with no opening. */
export const connectionFactsQuery = (environmentId: string, openingId: string | null, insidePort?: number, port?: number) => queryOptions({ queryKey: ['port-connection-facts', environmentId, openingId, port, insidePort] as const, retry: false, queryFn: ({ signal }): Promise<PortConnectionList> => {
 const query = { limit: 128, ...(openingId === null ? {} : { opening_id: openingId }), ...(insidePort === undefined ? {} : { inside_port: insidePort }), ...(port === undefined ? {} : { port }) };
 return ok(api.GET('/environments/{id}/ports/connections', { params: { path: { id: environmentId }, query }, signal }));
} });
export const hostPortsQuery = (id: string) => queryOptions({ queryKey: ['host-ports', id] as const, retry: false, queryFn: async (): Promise<View.HostPortsReview> => {
 const host: Host = await queryClient.query(hostQuery(id));
 const proposal = host.ports_proposal; const values = proposal?.values;
 return { current: host.ports === null ? null : { bind: host.ports.listen, address: host.ports.incoming_address, ranges: host.ports.ranges }, proposal: { bind: values?.listen ?? '', address: values?.incoming_address ?? '', ranges: values?.ranges ?? [] }, allocations: host.ports === null && host.ports_revision === 0 ? [] : null, revision: host.revision, acknowledged: null, can_edit: host.access.permissions.includes('host.manage'), reason: proposal?.reason ?? null, sources: proposal?.sources, as_of: proposal?.as_of, limits: proposal?.limits };
} });
export function saveHostPorts(id: string, ports: View.HostPorts | null, expected_revision: number): Promise<Host> { return ok(api.PATCH('/hosts/{id}', { params: { path: { id } }, body: { ports: ports === null ? null : { incoming_address: ports.address, listen: ports.bind, ranges: ports.ranges }, expected_revision } })); }

export function requestView(approval: PortApproval): View.PortRequest {
 const request = approval.request;
 if (approval.state === 'answered') throw new Error('a port permission cannot be answered with content');
 return { id: approval.id, revision: request.opening.revision, environment_id: request.environment.id, port: request.request.port, way: request.request.kind, reason: request.request.reason, agent: 'thread' in approval.requester ? approval.requester.thread.name : approval.requester.user.display_name, address: request.plan.target.address, audience: request.plan.audience.sentence, until: request.request.until ?? null, can_decide: approval.can_decide, state: approval.state === 'pending' ? 'pending' : approval.state === 'approved' ? 'allowed' : approval.state === 'denied' ? 'denied' : approval.state, transports: transportChoice(request.request.transports ?? ['tcp','udp']), link_carriers: request.request.kind === 'link' ? methodChoice(request.request.entry_methods ?? []) : null, approval };
}
export const portRequestsQuery = (id: string) => queryOptions({ queryKey: ['port-requests', id] as const, retry: false, queryFn: async ({ signal }): Promise<View.PortRequest[]> => {
 const page = await ok(api.GET('/approvals', { params: { query: { kind: 'port', scope: 'mine', state: 'pending' } }, signal }));
 return page.items.filter((approval): approval is PortApproval => approval.kind === 'port').filter((approval) => approval.request.environment.id === id).map(requestView);
} });
export async function decidePortRequest(request: View.PortRequest, decision: PortDecisionChoice, for_me_only: boolean): Promise<View.PortRequestResult> {
 const recipient: PortLinkRecipient = for_me_only ? 'person' : 'thread';
 const reply = await ok(api.POST('/approvals/{id}/decision', { params: { path: { id: request.id } }, body: { expected_revision: request.revision, decision, ...(request.way === 'link' ? { link_recipient: recipient } : {}) } }));
 // The link a person keeps is shown once and never kept in the request's view.
 const { opening_result: told, ...safe } = reply;
 const opening_result = told ?? null;
 const person = opening_result?.link?.recipient === 'person' ? opening_result.link : null;
 return { request: requestView(portApproval(safe)), opening: opening_result === null ? null : openingView(opening_result.opening), link_url_once: person?.url_once ?? null, browser_url_once: person?.browser_url_once ?? null, helper_link_once: person?.helper_link_once ?? null, delivered_to: opening_result?.link?.recipient === 'thread' ? 'thread' : person === null ? null : 'human' };
}

export async function readPortApproval(id: string): Promise<PortApproval> { return portApproval(await ok(api.GET('/approvals/{id}', { params: { path: { id } } }))); }
export const outsideOpeningsQuery = (id: string) => ({ queryKey: ['outside-port-openings', id] as const, initialPageParam: null as string | null, retry: false as const, queryFn: ({ pageParam, signal }: { pageParam: string | null; signal: AbortSignal }): Promise<PortOpeningPage> => ok(api.GET('/nodes/{id}/port-openings', { params: { path: { id }, ...(pageParam === null ? {} : { query: { cursor: pageParam } }) }, signal })), getNextPageParam: (page: PortOpeningPage) => page.next_cursor ?? undefined });
/** Grants the environment one audience's port capability: agents' requests of that kind then open without a card. */
export function grantPortCapability(id: string, key: string): Promise<PortCapability> { return ok(api.PUT('/environments/{id}/ports/capabilities/{key}', { params: { path: { id, key } } })); }
/** Takes a port capability back as every capability is taken back; what it opened stays until it is closed. */
export async function revokePortCapability(id: string, key: string): Promise<void> { await ok(api.DELETE('/environments/{id}/capabilities/{key}', { params: { path: { id, key } } })); }
