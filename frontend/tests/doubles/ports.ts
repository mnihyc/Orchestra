/** Ports component double: current observed listeners, guarded safe openings and deliberately transient secrets. */
import { randomUUID } from 'node:crypto';
import type { EnvironmentPort, EnvironmentPorts, HostPorts, HostTransports, ListeningPort, PortConnections, PortHttp, PortOpening, PortPublishPlan, PortReason } from '../../src/features/ports/presentation.ts';
import type { Environment, NodeRef, PermissionDef, Problem, ResolvedSetting } from '../../src/generated/api-types.ts';
import { ACME, RUNS_ON, userRef } from './fixtures.ts';
import { Refusal, type Answer, type Part, type Person, type World } from './world.ts';
import * as portsWire from './ports-wire.ts';
import { approvalItem, userMessage } from './thread-rows.ts';
import type * as Wire from '../../src/generated/api-types.ts';
import type { PortApproval, PortApprovalReply } from '../../src/features/ports/pending.ts';
import { methodChoice, programView, transportChoice } from '../../src/features/ports/wire.ts';

export const PORTS_PERMISSIONS: PermissionDef[] = [
 { name: 'ports.view', area: 'ports', level: 'view', on: ['environment'], label: 'View Ports', allows: 'See ports and use one for yourself.', holders: ['user', 'role', 'group', 'everyone'], options_schema: {}, profiles: false, with: null },
 { name: 'ports.manage', area: 'ports', level: 'manage', on: ['environment'], label: 'Manage Ports', allows: 'Create and close openings for others.', holders: ['user', 'role', 'group', 'everyone'], options_schema: {}, profiles: false, with: 'environment.manage' },
];
const MAX_OPEN = 16;
const LINK_MAX_SECONDS = 86_400;
const PLAN_LIFE_MS = 15 * 60_000;
const CONNECTIONS_LIMIT = 64;
const PORT_KEYS = { place: 'port_place', link: 'port_link', host: 'port_host' } as const;
const PORT_LABELS = { place: 'Ports for a place', link: 'Ports through a link', host: 'Ports on the host' } as const;
const DECLARATION: HostPorts = { bind: 'all', address: 'host.example', ranges: [{ from: 41000, to: 41015 }] };
const text = (value: unknown): string => typeof value === 'string' ? value : '';
const transports = (value: unknown): HostTransports => value === 'tcp' || value === 'udp' ? value : 'tcp_udp';
const ref = (id: string, kind: NodeRef['kind'], name: string): NodeRef => ({ id, kind, name, redacted: false });
function refuse(status: number, code: string, message: string, extra: Record<string, unknown> = {}): never { throw new Refusal(status, code as Problem['code'], message, extra); }
interface Scene {
 mode: string; domain: boolean; declaration: HostPorts | null; latency: number; listeners: ListeningPort[];
 drops?:Wire.PortSnapshot['udp_dropped']; known: Set<number>; race: boolean; lost: boolean; bindPending: boolean; bindFailure: string | null;
}
export class PortsWorld implements Part {
 private readonly world: World;
 private scenes = new Map<string, Scene>();
 private openings = new Map<string, PortOpening>();
 private requests = new Map<string, { signature: string; opening: string }>();
 private wirePlans = new Map<string, Wire.PortOpeningPlan>();
 private targetPrograms = new Map<string, { program: Wire.PortProgram; revision: number }>();
 private ownConnections = new Map<string, { port: number; value: PortConnections }>();
 private connections = new Map<string, PortConnections>();
 private portApprovals = new Map<string, PortApproval>();
 private agentEffects = new Map<string, { id: string; refusedUntil: number }>();
 private decisionFences = new Map<string, string>();
 /** Each environment's granted port capabilities, by key, with who gave each. */
 private grants = new Map<string, Map<string, ReturnType<typeof userRef>>>();
 private confirmations = new Map<string, { signature: string; opening: string }>();
 readonly mutations: { act: string; body: Record<string, unknown> }[] = [];
 constructor(world: World) { this.world = world; }
 reset(): void { this.scenes.clear(); this.openings.clear(); this.requests.clear(); this.wirePlans.clear(); this.targetPrograms.clear(); this.connections.clear(); this.ownConnections.clear(); this.portApprovals.clear(); this.confirmations.clear(); this.agentEffects.clear(); this.decisionFences.clear(); this.grants.clear(); this.mutations.length = 0; }
 private scene(id: string): Scene {
  let scene = this.scenes.get(id);
  if (scene === undefined) { scene = { mode: 'empty', domain: true, declaration: null, latency: 0, listeners: [], known: new Set(), race: false, lost: false, bindPending: false, bindFailure: null }; this.scenes.set(id, scene); }
  return scene;
 }
 private permissions(person: Person, id: string): string[] { return this.world.access.permissionsOnEnvironment(person, id); }
 private require(person: Person, id: string, permission: string): void {
  if (!this.permissions(person, id).includes(permission)) {
   const owner = userRef(this.world.people.get('ada')!.me); let environment: Environment | null = null;
   try { environment = this.world.resources.handle(person, 'GET', `/environments/${id}`, new URLSearchParams(), {})?.body as Environment; } catch { /* No readable metadata is added to a refusal. */ }
   refuse(403, 'authz.denied', 'Ports access changed. Ask Ada Lovelace.', { required: permission, holders: [{ kind: 'user', user: owner }], override: environment?.access.administers === true, missing: [{ object: { kind: 'environment', id, name: environment?.name ?? null, owner }, permissions: [permission], ask: [owner], ask_more: 0, closed: null, options: null, override: environment?.access.administers === true }] });
  }
 }
 private setting(key: string, value: unknown): ResolvedSetting { return { key: `ports.${key}`, value, source: { scope: 'org', node: ref(ACME.org, 'org', 'Acme') }, own: null, locked_by: null, writable: false, parts: [] }; }
 private reason(act: string, code: string, message: string): PortReason { return { act, code, message, scope: ref(ACME.org, 'org', 'Acme'), holders: [{ kind: 'user', user: userRef(this.world.people.get('ada')!.me) }] }; }
 exposed(id: string): boolean { return [...this.openings.values()].some((opening) => opening.environment_id === id && opening.desired && opening.state !== 'closed' && opening.state !== 'expired' && opening.state !== 'waiting'); }
 private expire(): void { for (const opening of this.openings.values()) if (opening.until !== null && Date.parse(opening.until) <= Date.now() && opening.state !== 'closed') { opening.state = 'expired'; opening.desired = false; opening.revision += 1; } }
 private port(person: Person, id: string, number: number): EnvironmentPort {
  this.require(person, id, 'ports.view');
  const scene = this.scene(id);
  const listeners = scene.listeners.filter((row) => row.port === number).map((row) => ({ ...row, process: row.process === null ? null : { ...row.process, thread: this.world.places.denied.get(person.me.username)?.has('thread.view') === true ? null : row.process.thread } }));
  const manage = this.permissions(person, id).includes('ports.manage');
  const name = scene.domain ? `e${id.replaceAll('-', '')}-p${number}.ports.example` : null;
  const internal = listeners.some((listener) => listener.internal); const program = this.targetPrograms.get(`${id}:${number}`);
  const openings = [...this.openings.values()].filter((opening) => opening.environment_id === id && opening.port === number).map((opening) => ({ ...opening, can: { ...opening.can, close: manage, change: manage } }));
  return { counts: openings.reduce((sum, opening) => sum + (opening.connection_count ?? 0), 0) + (this.ownConnections.get(id)?.port === number ? this.ownConnections.get(id)?.value.count ?? 0 : 0), program: program === undefined ? undefined : programView(program.program), revision: program?.revision ?? 0, port: number, name, url: name === null ? null : `https://${name}`, helper_command: `orch-connector port forward --environment ${id} --port ${number} --transport tcp --token-stdin`, listeners, openings, internal, state: scene.mode === 'offline' ? 'stopped' : scene.mode === 'unsupported' ? 'unavailable' : listeners.length > 0 ? 'listening' : 'nothing_listens', reason: scene.mode === 'offline' ? 'The environment is stopped. Its openings are kept.' : internal ? 'This private control port stays Internal.' : null, can: { enter: !internal && !['policy_off', 'local_off', 'connection_limit'].includes(scene.mode), let_more_in: manage && !internal && scene.mode !== 'port_limit', here: !internal && this.permissions(person, id).includes('browser.manage') } };
 }
 snapshot(person: Person, id: string): EnvironmentPorts {
  this.require(person, id, 'ports.view'); this.expire();
  const scene = this.scene(id); const manage = this.permissions(person, id).includes('ports.manage');
  const enabled = !['policy_off', 'local_off'].includes(scene.mode);
  const links = scene.mode !== 'links_off'; const publish = scene.mode !== 'publish_off';
  const reasons: PortReason[] = [];
  if (!enabled) reasons.push(this.reason('enter', 'ports.not_allowed', scene.mode === 'local_off' ? 'Ports are disabled on Ada’s machine. Ask its owner or use its local console.' : 'Through Orch is off at Acme.'));
  if (!links) reasons.push(this.reason('link', 'ports.not_allowed', '“Ports through a link” (port_link) is denied by capability.policy at Acme.'));
  if (!publish) reasons.push(this.reason('host', 'ports.not_allowed', '“Ports on the host” (port_host) is denied by capability.policy at Acme.'));
  if (!scene.domain) reasons.push(this.reason('browser', 'ports.domain_missing', scene.mode === 'certificate' ? 'The Ports wildcard certificate is unavailable. The helper remains available.' : 'No Ports domain is configured. Use Here or the helper.'));
  if (scene.declaration === null) reasons.push(this.reason('host', 'ports.no_range', 'host-1 has no saved reached address or port range.'));
  if (scene.mode === 'port_limit') reasons.push(this.reason('host', 'ports.too_many', '16 of 16 ports are open to others. Close an opening before creating another.'));
  if (scene.mode === 'connection_limit') reasons.push(this.reason('connection', 'ports.too_many', 'The operator’s connection limit is reached: 64 active, 16 waiting.'));
  const ports = [...new Set([...scene.known, ...scene.listeners.map((row) => row.port), ...[...this.openings.values()].filter((opening) => opening.environment_id === id).map((opening) => opening.port)])].sort((a, b) => a - b).map((number) => this.port(person, id, number));
  return { snapshot: { environment_id: id, generation: scene.listeners[0]?.generation ?? 1, as_of: '2026-10-13T09:00:00.000Z', udp_dropped:scene.drops, discovery: scene.mode === 'offline' ? 'offline' : scene.mode === 'unsupported' ? 'unsupported' : 'native', reason: scene.mode === 'offline' ? 'Current listening is unknown; this observation is old.' : scene.mode === 'unsupported' ? 'Scoped socket ownership is not supported on this platform.' : null, listeners: [] }, listeners: ports.flatMap((row) => row.listeners), ports, openings: ports.flatMap((row) => row.openings), policy: { enabled: this.setting('enabled', enabled), max_open: this.setting('max_open', MAX_OPEN), link_max_secs: this.setting('link_max_secs', LINK_MAX_SECONDS) }, capabilities: this.capabilities(person, id, links, publish), host_ports: scene.declaration, ports_exposed: this.exposed(id), can: { open_place: manage && enabled, make_link: manage && enabled && links, publish: manage && enabled && publish && scene.declaration !== null }, reasons, browser_available: scene.domain };
 }
 private granted(id: string, kind: PortOpening['way']): boolean { return this.grants.get(id)?.has(PORT_KEYS[kind]) === true; }
 private admission(id: string, kind: PortOpening['way']): Wire.PortOpeningAdmission { return { capability: PORT_KEYS[kind], source: 'grant', scope: null, by: this.grants.get(id)?.get(PORT_KEYS[kind]) ?? null }; }
 /** The three audiences' capabilities as the server answers them: ask, or deny where a scene turns a kind off; held where granted. */
 private capabilities(person: Person, id: string, links: boolean, publish: boolean): Wire.PortCapability[] {
  const manage = this.permissions(person, id).includes('ports.manage');
  return (['place', 'link', 'host'] as const).map((kind) => {
   const decision = kind === 'link' && !links || kind === 'host' && !publish ? 'deny' as const : 'ask' as const;
   const granted = this.granted(id, kind);
   return { key: PORT_KEYS[kind], kind, label: PORT_LABELS[kind], risk: kind === 'place' ? 'medium' as const : 'high' as const, offered: true, granted, from_template: false, decision, approver_scope: 'environment' as const, admission: granted && decision !== 'deny' ? this.admission(id, kind) : null, can_grant: manage && !granted && decision !== 'deny', can_revoke: manage && granted };
  });
 }
 private guard(person: Person, id: string, port: number, way: PortOpening['way']): void {
  this.require(person, id, 'ports.manage');
  if (!Number.isInteger(port) || port < 1 || port > 65535) refuse(400, 'request.invalid', 'A port from 1 to 65535 is required.');
  const data = this.snapshot(person, id);
  if (this.port(person, id, port).internal) refuse(403, 'ports.not_allowed', 'Internal control ports cannot be opened.');
  const allowed = way === 'place' ? data.can.open_place : way === 'link' ? data.can.make_link : data.can.publish;
  if (!allowed) refuse(403, 'ports.not_allowed', 'Acme does not allow this opening. Ask Ada Lovelace.', { holders: this.reason(way, '', '').holders, scope: ref(ACME.org, 'org', 'Acme') });
  if (this.scene(id).mode === 'port_limit') refuse(409, 'ports.too_many', '16 of 16 ports are already open to others.');
 }
 private create(person: Person, id: string, port: number, way: PortOpening['way'], body: Record<string, unknown>, plan?: PortPublishPlan): { opening: PortOpening; fresh: boolean } {
  const clientId = text(body.client_id); const signature = JSON.stringify({ id, port, way, body }); const prior = this.requests.get(clientId);
  if (prior !== undefined) { if (prior.signature !== signature) refuse(409, 'conflict.revision', 'The client id belongs to another request.'); return { opening: this.openings.get(prior.opening)!, fresh: false }; }
  const scene = this.scene(id); const portRow = this.port(person, id, port); const state = plan !== undefined && scene.bindPending ? 'opening' : plan !== undefined && scene.bindFailure !== null ? 'failed' : scene.mode === 'offline' ? 'stopped' : portRow.listeners.length === 0 ? 'nothing_listens' : 'open';
  const placeRow = this.world.places.rowOf(text(body.place_id));
  const place = way === 'place' && placeRow !== undefined ? ref(placeRow.id, placeRow.kind, placeRow.name) : null;
  const http = typeof body.http === 'object' && body.http !== null ? body.http as PortHttp : { scheme: 'http' as const, host_header: null };
  const opening: PortOpening = { id: randomUUID(), environment_id: id, port, way, transports: plan?.transports ?? transports(body.transports), opened_by: userRef(person.me), requested_by: userRef(person.me), allowed_by: userRef(person.me), label: text(body.label) || (way === 'link' ? 'Service link' : ''), opened_at: new Date().toISOString(), until: way === 'link' ? text(body.until) : null, desired: true, state, reason: scene.bindFailure, name: portRow.name, url: way === 'host' ? plan?.address ?? null : portRow.url, host_allocated: way === 'host' && state !== 'opening' && state !== 'failed' && state !== 'stopped', host: plan?.host ?? null, publisher: plan?.publisher ?? null, host_port: plan?.host_port ?? null, fixed_host_port: plan?.fixed ?? false, revision: 1, place, viewers: way === 'place' ? 7 : null, link_carriers: way === 'link' ? body.link_carriers === 'browser' || body.link_carriers === 'helper' ? body.link_carriers : 'both' : null, http, connection_count: 0, can: { enter: true, close: true, change: true } };
  this.openings.set(opening.id, opening); this.requests.set(clientId, { signature, opening: opening.id }); this.mutations.push({ act: way, body });
  return { opening, fresh: true };
 }
 private seedApproval(environmentId: string, request: Wire.PortOpeningRequest, threadId = ACME.repro): PortApproval {
  const person = this.world.people.get('ada')!; const key = `${threadId}:${environmentId}:${JSON.stringify({ ...request, reason: undefined })}`;
  const prior = this.agentEffects.get(key);
  if (prior !== undefined) { const held = this.portApprovals.get(prior.id); if (held !== undefined && (held.state !== 'denied' || prior.refusedUntil > Date.now())) return held; }
  const target = portsWire.target(this.port(person, environmentId, request.port), environmentId);
  const environment = this.world.resources.handle(person, 'GET', `/environments/${environmentId}`, new URLSearchParams(), {})?.body as Environment;
  const placeRow = request.kind === 'place' ? this.world.places.rowOf(request.place_id ?? this.world.places.rowOf(threadId)?.parent_id ?? '') : undefined;
  const place = placeRow === undefined ? null : ref(placeRow.id, placeRow.kind, placeRow.name);
  const resolved = { ...request, ...(place === null ? {} : { place_id: place.id }), transports: request.transports ?? ['tcp','udp'] as Wire.PortTransport[], entry_methods: request.kind === 'link' ? request.entry_methods ?? (this.scene(environmentId).domain ? ['browser','helper'] : ['helper']) : undefined };
  const waiting:PortOpening = { id: randomUUID(), environment_id: environmentId, port: request.port, way: request.kind, transports: transportChoice(resolved.transports), opened_by: null, requested_by: null, requested_by_thread: ref(threadId, 'thread', this.world.places.rowOf(threadId)?.name ?? 'Removed thread'), allowed_by: null, label: request.label ?? 'Webhook', link_recipient: 'thread', opened_at: new Date().toISOString(), until: request.until ?? null, desired:false, state:'waiting', reason:null, name:target.address, url:target.address, host:null, host_port:request.host_port??null, fixed_host_port:request.host_port!==undefined, host_allocated:false, revision:1, place, viewers:null, link_carriers:request.kind==='link'?methodChoice(resolved.entry_methods??[]):null, http:programView(request.program??target.program), connection_count:0, can:{enter:false,close:false,change:false} };
  this.openings.set(waiting.id,waiting);
  const sentence = request.kind === 'place' ? `Everyone who may view ${place?.name??'this place'}, including guests, can use this port after signing in.` : request.kind === 'link' ? `Anyone with this link can use this port until ${request.until??''}.` : `Anybody who reaches host.example:${request.host_port??41000} reaches this. Orch does not ask them who they are.`;
  const disclosure = request.kind === 'link' ? 'This link is given to the agent and is readable in this thread.' : null;
  const plan: Wire.PortOpeningPlan = { preflight_id:randomUUID(),expires_at:new Date(Date.now()+PLAN_LIFE_MS).toISOString(),target,program_revision:target.revision,request:resolved,audience:{place,count:place===null?null:7,sentence},candidate:null,disclosure,blockers:[] };
  const approval:PortApproval = { id:randomUUID(),kind:'port',state:'pending',title:`Open port ${request.port}`,node:ref(threadId,'thread',this.world.places.rowOf(threadId)?.name??'Removed thread'),object:{kind:'environment',id:environmentId,name:environment.name},thread_id:threadId,item_id:randomUUID(),requester:{thread:{id:threadId,name:'Scripted agent'}},request:{environment:{id:environment.id,name:environment.name,owner:environment.owner,address:environment.address,placement:environment.placement,status:environment.status,ports_exposed:this.exposed(environment.id)},request:resolved,opening:portsWire.opening(waiting),plan,link_recipient:'thread',recipient_warning:disclosure,can_grant:!this.granted(environmentId,request.kind)},required_permission:'ports.manage',approver_scope:null,batch_id:null,can_decide:true,decided_by:null,decided_automatically:false,decided_at:null,decision:null,expires_at:new Date(Math.min(Date.now()+24*3_600_000,request.kind==='link'?Date.parse(request.until??''):Infinity)).toISOString(),created_at:new Date().toISOString() };
  this.portApprovals.set(approval.id,approval); this.agentEffects.set(key,{id:approval.id,refusedUntil:0});
  this.world.threads.addApproval(approval);
  this.world.threads.emit(threadId,{items:[approvalItem(approval.item_id??randomUUID(),approval.id,{status:'pending'})]});
  return approval;
 }
 private approvalView(person: Person, approval: PortApproval): PortApproval { return { ...approval, can_decide: this.permissions(person,approval.request.environment.id).includes('ports.manage') }; }
 private linkDelivery(opening:PortOpening,person:Person,recipient:'thread'|'person'):Wire.PortLinkDelivery {
  const proof=randomUUID();const root=`https://orch.example/api/v1/ports/links/${opening.id}#${proof}`;
  return {recipient,url_once:root,browser_url_once:opening.link_carriers!=='helper'&&opening.url!==null?`${opening.url}?orch_token=${proof}`:null,helper_link_once:opening.link_carriers!=='browser'?root:null,delivered_to:recipient==='thread'?opening.requested_by_thread?.id??'':person.me.id};
 }
 async handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Promise<Answer | null> {
  this.expire();

  if(path==='/approvals'&&method==='GET'&&(query.get('kind')==='port'||this.portApprovals.size>0)) {
   const ordinary = query.get('kind')==='port'?null:this.world.notifications.handle(person,method,path,query,body);
   const base = ordinary?.body as {items:unknown[];next_cursor:string|null}|undefined;
   const ports=[...this.portApprovals.values()].filter((approval)=>(query.get('kind')===null||query.get('kind')==='port')&&(query.get('state')==='decided'?approval.state!=='pending':approval.state==='pending')&&(query.get('scope')==='requested'?false:this.permissions(person,approval.request.environment.id).includes('ports.manage'))).map((approval)=>this.approvalView(person,approval));
   const all=[...(base?.items??[]).filter((value)=>typeof value!=='object'||value===null||!('kind' in value)||value.kind!=='port'),...ports];
   return {status:200,body:{items:all,next_cursor:base?.next_cursor??null}};
  }
  const approvalMatch=/^\/approvals\/([^/]+)(\/decision)?$/.exec(path);
  if(approvalMatch!==null&&this.portApprovals.has(approvalMatch[1]??'')) {
   const approval=this.portApprovals.get(approvalMatch[1]??'')!;const current=this.approvalView(person,approval);
   if(!current.can_decide&&!(approval.thread_id!==null&&this.world.places.access(person,approval.thread_id).permissions.includes('thread.view'))) refuse(403,'authz.denied','This request is not readable.');
   if(method==='GET'&&approvalMatch[2]===undefined) {
    if(current.can_decide&&approval.state==='pending'&&Date.parse(approval.request.plan.expires_at)<=Date.now()) { approval.request.plan.expires_at=new Date(Date.now()+PLAN_LIFE_MS).toISOString();approval.request.plan.target=portsWire.target(this.port(person,approval.request.environment.id,approval.request.request.port),approval.request.environment.id);approval.request.plan.program_revision=approval.request.plan.target.revision;approval.request.opening.revision+=1; }
    return {status:200,body:this.approvalView(person,approval)};
   }
   if(method==='POST'&&approvalMatch[2]==='/decision') {
    this.require(person,approval.request.environment.id,'ports.manage');
    if(!['once','grant','deny'].includes(text(body.decision))||Object.keys(body).some((key)=>!['decision','link_recipient','expected_revision'].includes(key))) refuse(400,'request.invalid','Choose Allow once, Grant to this environment, or Deny.');
    if(approval.state!=='pending') { if(this.decisionFences.get(approval.id)!==JSON.stringify(body)) refuse(409,'approval.already_decided','This request was already decided.',{current}); return {status:200,body:{...current,opening_result:{opening:approval.request.opening,request:null,link:null}}}; }
    if(body.expected_revision!==approval.request.opening.revision) refuse(409,'conflict.revision','This request changed. Refresh its review.',{current});
    if(body.decision!=='deny'&&Date.parse(approval.request.plan.expires_at)<=Date.now()) refuse(409,'conflict.revision','This review is old. Refresh it on the same card.',{current});
    const request=approval.request.request,id=approval.request.environment.id;const opening=this.openings.get(approval.request.opening.id)!;
    if(body.decision!=='deny') this.guard(person,id,request.port,request.kind);
    const port=this.port(person,id,request.port);
    opening.desired=body.decision!=='deny';opening.state=body.decision==='deny'?'refused':port.state==='stopped'?'stopped':port.listeners.length===0?'nothing_listens':'open';opening.allowed_by=userRef(person.me);opening.opened_by=userRef(person.me);opening.revision+=1;opening.reason=body.decision==='deny'?'The current manager refused this opening.':null;
    if(body.decision==='grant'){const held=this.grants.get(id)??new Map<string,ReturnType<typeof userRef>>();if(!held.has(PORT_KEYS[request.kind]))held.set(PORT_KEYS[request.kind],userRef(person.me));this.grants.set(id,held);opening.admission=this.admission(id,request.kind);}
    approval.state=body.decision==='deny'?'denied':'approved';approval.decided_by=userRef(person.me);approval.decided_at=new Date().toISOString();approval.decision={decision:body.decision};approval.request.opening=portsWire.opening(opening);this.decisionFences.set(approval.id,JSON.stringify(body));
    if(body.decision==='deny') for(const effect of this.agentEffects.values()) if(effect.id===approval.id) effect.refusedUntil=Date.now()+3_600_000;
    const recipient=body.link_recipient==='person'?'person':'thread';opening.link_recipient=recipient;approval.request.link_recipient=recipient;approval.request.opening=portsWire.opening(opening);const delivery=request.kind==='link'&&approval.state==='approved'?this.linkDelivery(opening,person,recipient):null;
    if(approval.thread_id!==null){const item=approval.item_id===null?undefined:this.world.threads.itemOf(approval.thread_id,approval.item_id);const notice=`Your request for port ${request.port} in ${approval.request.environment.name}, for ${approval.request.plan.audience.sentence}, is ${opening.state}. ${opening.reason??''} ${recipient==='thread'&&delivery!==null?delivery.browser_url_once??delivery.url_once??'':delivery!==null?'The deciding person holds the link; it exists until '+(request.until??'')+'.':''}`;this.world.threads.emit(approval.thread_id,{items:[...(item===undefined?[]:[{...item,status:approval.state==='approved'?'approved' as const:'denied' as const}]),userMessage(randomUUID(),notice,{author:userRef(person.me),source:'system'})]});}
    return {status:200,body:{...this.approvalView(person,approval),opening_result:{opening:approval.request.opening,request:null,link:delivery}} satisfies PortApprovalReply};
   }
  }
  const producer=/^\/threads\/([^/]+)\/port-requests$/.exec(path);
  if(producer!==null&&method==='POST') {
   const thread=producer[1]??'';this.world.places.need(person,thread,'thread.interact');const environment=this.world.resources.refsOf(this.world.resources.bindingOf(thread)).environment;
   if(environment===null) refuse(409,'ports.unavailable','This thread has no environment.');
   if(Object.keys(body).some((key)=>!['port','kind','place_id','label','transports','entry_methods','until','host_port','program','reason'].includes(key))) refuse(400,'request.invalid','The request has an unknown member.');
   const request=body as unknown as Wire.PortOpeningRequest;if(!Number.isInteger(request.port)||request.port<1||request.port>65535||!['place','link','host'].includes(request.kind)||typeof request.reason!=='string') refuse(400,'request.invalid','A port, audience and reason are required.');
   if(this.granted(environment.id,request.kind)&&this.capabilities(person,environment.id,this.scene(environment.id).mode!=='links_off',this.scene(environment.id).mode!=='publish_off').find((one)=>one.kind===request.kind)?.decision!=='deny'){
    const port=this.port(this.world.people.get('ada')!,environment.id,request.port);const placeRow=request.kind==='place'?this.world.places.rowOf(request.place_id??this.world.places.rowOf(thread)?.parent_id??''):undefined;
    const opened:PortOpening={ id:randomUUID(),environment_id:environment.id,port:request.port,way:request.kind,transports:transportChoice(request.transports??['tcp','udp']),opened_by:null,requested_by:null,requested_by_thread:ref(thread,'thread',this.world.places.rowOf(thread)?.name??'Removed thread'),allowed_by:null,admission:this.admission(environment.id,request.kind),label:request.label??(request.kind==='link'?'Webhook':''),link_recipient:'thread',opened_at:new Date().toISOString(),until:request.until??null,desired:true,state:port.state==='stopped'?'stopped':port.listeners.length===0?'nothing_listens':'open',reason:null,name:port.name,url:port.url,host:null,host_port:request.host_port??null,fixed_host_port:request.host_port!==undefined,host_allocated:false,revision:1,place:placeRow===undefined?null:ref(placeRow.id,placeRow.kind,placeRow.name),viewers:null,link_carriers:request.kind==='link'?methodChoice(request.entry_methods??['helper']):null,http:programView(request.program??{http_scheme:'http',host_header:null,proxy_protocol:false}),connection_count:0,can:{enter:true,close:true,change:true} };
    this.openings.set(opened.id,opened);this.mutations.push({act:'automatic',body});
    return {status:200,body:{opening:portsWire.opening(opened),request:null,link:null} satisfies Wire.PortOpenReply};
   }
   const approval=this.seedApproval(environment.id,request,thread);
   return {status:200,body:{opening:approval.request.opening,request:{id:approval.id,state:approval.state,can_decide:this.permissions(person,environment.id).includes('ports.manage'),expires_at:approval.expires_at},link:null} satisfies Wire.PortOpenReply};
  }
  const grant=/^\/environments\/([^/]+)\/ports\/capabilities\/(port_place|port_link|port_host)$/.exec(path);
  if(grant!==null&&method==='PUT'){
   const id=grant[1]??'';this.require(person,id,'ports.manage');const scene=this.scene(id);
   const capability=this.capabilities(person,id,scene.mode!=='links_off',scene.mode!=='publish_off').find((one)=>one.key===grant[2]);
   if(capability===undefined) refuse(400,'request.invalid','A port capability is port_place, port_link or port_host.');
   if(capability.decision==='deny') refuse(403,'ports.not_allowed',`“${capability.label}” is denied by capability.policy at Acme.`);
   const held=this.grants.get(id)??new Map<string,ReturnType<typeof userRef>>();if(!held.has(capability.key))held.set(capability.key,userRef(person.me));this.grants.set(id,held);this.mutations.push({act:'grant',body:{key:capability.key}});
   return {status:200,body:this.capabilities(person,id,scene.mode!=='links_off',scene.mode!=='publish_off').find((one)=>one.key===capability.key)};
  }
  const revoke=/^\/environments\/([^/]+)\/capabilities\/(port_place|port_link|port_host)$/.exec(path);
  if(revoke!==null&&method==='DELETE'){
   const id=revoke[1]??'';this.require(person,id,'ports.manage');
   if(this.grants.get(id)?.delete(revoke[2]??'')!==true) refuse(404,'not_found','The environment was not given that capability.');
   this.mutations.push({act:'revoke',body:{key:revoke[2]}});
   return {status:202,body:null};
  }
  let match=/^\/environments\/([^/]+)\/ports$/.exec(path);
  if(match!==null&&method==='GET') {
   const id=match[1]??'', scene=this.scene(id); if(scene.latency>0) await new Promise((resolve)=>setTimeout(resolve,scene.latency)); if(scene.mode==='failed') refuse(503,'ports.unavailable','The current port snapshot could not be read.');
   const data=this.snapshot(person,id); const requested=query.get('port');
   if(requested!==null&&!data.ports.some((port)=>port.port===Number(requested))) data.ports.push(this.port(person,id,Number(requested)));
   return {status:200,body:{...portsWire.environment(data),...(scene.mode==='port_limit'?{used:MAX_OPEN}:{} )}};
  }
  match=/^\/environments\/([^/]+)\/settings$/.exec(path);
  if(match!==null&&method==='GET') {
   const id=match[1]??''; const ordinary=this.world.resources.handle(person,method,path,query,body); const settings=this.snapshot(person,id).policy;
   return {status:200,body:[...(Array.isArray(ordinary?.body)?ordinary.body as ResolvedSetting[]:[]),...Object.values(settings)]};
  }
  match=/^\/environments\/([^/]+)\/ports\/(\d+)(?:\/(program|openings\/preflight|openings))?$/.exec(path);
  if(match!==null) {
   const id=match[1]??'',number=Number(match[2]),act=match[3]; const scene=this.scene(id); const key=`${id}:${number}`;
   const row=this.port(person,id,number); const existingProgram=this.targetPrograms.get(key); if(existingProgram!==undefined){row.program=programView(existingProgram.program);row.revision=existingProgram.revision;}
   const target=portsWire.target(row,id);
   if(act===undefined&&method==='GET') return {status:200,body:target};
   if(act==='program'&&method==='PATCH') {
    this.require(person,id,'ports.manage'); if(body.expected_revision!==target.revision) refuse(409,'conflict.revision','The program options changed. Review again.');
    const program=body.program as Wire.PortProgram; this.targetPrograms.set(key,{program,revision:target.revision+1}); for(const opening of this.openings.values()) if(opening.environment_id===id&&opening.port===number) opening.http=programView(program); target.program=program;target.revision+=1;
    return {status:200,body:target};
   }
   if(act==='openings/preflight'&&method==='POST') {
    const allowed=new Set(['port','kind','place_id','label','transports','entry_methods','until','host_port','program','reason','replacement_of','close_replaced']);
    if(Object.keys(body).some((name)=>!allowed.has(name))) refuse(400,'request.invalid','The opening request has an unknown member.');
    const request=body as unknown as Wire.PortOpeningRequest;
    if(request.port!==number||!['place','link','host'].includes(request.kind)||typeof request.reason!=='string') refuse(400,'request.invalid','A port, audience and reason are required.');
    this.guard(person,id,number,request.kind);
    const blockers:Wire.PortOpeningPlan['blockers']=[]; let place:Wire.PortOpeningPlan['audience']['place']=null;let count:number|null=null;let sentence:string;let candidate:Wire.PortOpeningPlan['candidate']=null;
    const program=request.program??target.program;
    if(program.host_header!==null&&(!/^[a-zA-Z0-9_.-]+(?::\d+)?$/.test(program.host_header))) blockers.push({code:'request.invalid',message:'Expected Host must be a hostname with an optional port.'});
    if(request.kind==='place') {
     const chosen=this.world.places.rowOf(request.place_id??'');if(chosen===undefined||!['org','project','topic'].includes(chosen.kind)) refuse(404,'not_found','Choose a current readable place.');this.world.places.need(person,chosen.id,`${chosen.kind}.view`);
     place=ref(chosen.id,chosen.kind,chosen.name);count=7;sentence=`Everyone who may view ${chosen.name}, including guests, can use this port after signing in.`;
    } else if(request.kind==='link') {
     const until=Date.parse(request.until??'');if(!Number.isFinite(until)||until<=Date.now()||until>Date.now()+LINK_MAX_SECONDS*1000) blockers.push({code:'ports.not_allowed',message:'Choose a future lapse within 86400 seconds.'});
     const methods=request.entry_methods??(scene.domain?['browser','helper']:['helper']);if(!scene.domain&&methods.includes('browser')) blockers.push({code:'ports.domain_missing',message:'A browser link requires the Ports domain. The helper remains available.'});
     sentence=`Anyone with this link can use this port until ${request.until??''}.`;
     if(typeof request.replacement_of==='string'){const previous=this.openings.get(request.replacement_of);if(previous?.environment_id!==id||previous.port!==number||previous.way!=='link') blockers.push({code:'request.invalid',message:'Choose an existing link of this port to replace.'});}
    } else {
     const declaration=scene.declaration;if(declaration===null) refuse(409,'ports.no_range','The host has no saved declaration.');
     const publisher=this.world.resources.publisherOf(person,id);
     if(publisher===null) refuse(409,'ports.no_range','No publishing machine is selected.');
     const hostPort=request.host_port??declaration.ranges[0]!.from;
     if(!declaration.ranges.some((range)=>hostPort>=range.from&&hostPort<=range.to)) blockers.push({code:'ports.no_range',message:'The fixed port is outside the declared range.'});
     candidate={host:publisher.host,machine:publisher.machine,host_port:hostPort,fixed:request.host_port!==undefined,host_revision:publisher.revision,local_limits_revision:1};
     sentence=`Anybody who reaches ${declaration.address}:${hostPort} reaches this. Orch does not ask them who they are.`;
    }
    const plan:Wire.PortOpeningPlan={preflight_id:randomUUID(),expires_at:new Date(Date.now()+PLAN_LIFE_MS).toISOString(),target,program_revision:target.revision,request:{...request,program},audience:{place,count,sentence},candidate,disclosure:null,blockers};this.wirePlans.set(plan.preflight_id,plan);scene.race=false;
    return {status:200,body:plan};
   }
   if(act==='openings'&&method==='POST') {
    this.require(person,id,'ports.manage');
    if(Object.keys(body).some((field)=>!['client_id','preflight_id'].includes(field))||typeof body.client_id!=='string') refuse(400,'request.invalid','A client id and stored preflight are required.');
    const fenceKey=`${id}:${person.me.id}:${body.client_id}`,signature=JSON.stringify(body),prior=this.confirmations.get(fenceKey);
    if(prior!==undefined){if(prior.signature!==signature) refuse(409,'conflict.revision','The client id belongs to another request.');const opening=this.openings.get(prior.opening)!;return {status:200,body:{opening:portsWire.opening(opening),request:null,link:opening.way==='link'?{recipient:'person',url_once:null,browser_url_once:null,helper_link_once:null,delivered_to:person.me.id}:null} satisfies Wire.PortOpenReply};}

    const plan=this.wirePlans.get(text(body.preflight_id));if(plan?.target.environment_id!==id||plan.target.port!==number||Date.parse(plan.expires_at)<=Date.now()||scene.race) refuse(409,'conflict.revision','The reviewed candidate changed or expired. Review again.');
    if(plan.blockers.length>0) refuse(409,plan.blockers[0]!.code,plan.blockers[0]!.message);
    this.guard(person,id,number,plan.request.kind);
    const currentProgram=this.targetPrograms.get(key);if((currentProgram?.revision??0)!==plan.program_revision) refuse(409,'conflict.revision','The program options changed. Review again.');
    const request=plan.request; const legacy={label:request.label,client_id:fenceKey,until:request.until,place_id:request.place_id,transports:transportChoice(request.transports??['tcp','udp']),link_carriers:methodChoice(request.entry_methods??(scene.domain?['browser','helper']:['helper'])),http:programView(request.program??target.program)};
    const hostPlan=plan.candidate===null?undefined:{preflight_id:plan.preflight_id,expires_at:plan.expires_at,environment_id:id,port:number,transports:transportChoice(request.transports??['tcp','udp']),host:plan.candidate.host,publisher:plan.candidate.machine,host_ports_revision:plan.candidate.host_revision,host_port:plan.candidate.host_port,fixed:plan.candidate.fixed,address:`${scene.declaration?.address??''}:${plan.candidate.host_port}`,audience:plan.audience.sentence,blockers:[]} satisfies PortPublishPlan;
    if(plan.candidate!==null){const publisher=this.world.resources.publisherOf(person,id);if(publisher?.machine.id!==plan.candidate.machine.id||publisher.revision!==plan.candidate.host_revision) refuse(409,'conflict.revision','The machine declaration changed. Review again.');}
    const created=this.create(person,id,number,request.kind,legacy,hostPlan);
    this.confirmations.set(fenceKey,{signature,opening:created.opening.id});
    if(created.fresh){this.targetPrograms.set(key,{program:request.program??target.program,revision:target.revision+1});for(const opening of this.openings.values()) if(opening.environment_id===id&&opening.port===number) opening.http=programView(request.program??target.program);if(typeof request.replacement_of==='string'&&request.close_replaced===true){const previous=this.openings.get(request.replacement_of);if(previous!==undefined){previous.state='closed';previous.desired=false;previous.revision+=1;}}}
    const delivered=created.fresh&&request.kind==='link'; const proof=delivered?randomUUID():null;
    const helper=proof===null?null:`https://orch.example/api/v1/ports/links/${created.opening.id}#${proof}`;
    const browser=proof===null||!scene.domain?null:`${target.address??''}?orch_token=${proof}`;
    const link:Wire.PortLinkDelivery|null=request.kind!=='link'?null:{recipient:'person',url_once:helper,browser_url_once:(request.entry_methods??[]).includes('browser')?browser:null,helper_link_once:(request.entry_methods??['helper']).includes('helper')?helper:null,delivered_to:person.me.id};
    if(created.fresh&&scene.lost){scene.lost=false;refuse(503,'ports.unavailable','The response was lost after the link was created.');}
    return {status:created.fresh?201:200,body:{opening:portsWire.opening(created.opening),request:null,link} satisfies Wire.PortOpenReply};
   }
  }
  match=/^\/environments\/([^/]+)\/ports\/connections$/.exec(path);
  if(match!==null&&method==='GET') {
   const id=match[1]??'';this.require(person,id,'ports.view');const manage=this.permissions(person,id).includes('ports.manage');
   const wantedOpening = query.get('opening_id'); const wantedPort = query.get('port');
   const parts = [...this.connections.entries()].flatMap(([openingId,value])=>{const selected=this.openings.get(openingId);if(selected?.environment_id!==id||wantedOpening!==null&&openingId!==wantedOpening||wantedPort!==null&&selected.port!==Number(wantedPort))return [];return [portsWire.connections(value,id,openingId,selected.port,manage)];});
   const own = this.ownConnections.get(id); if(own !== undefined && wantedOpening === null && (wantedPort === null || Number(wantedPort) === own.port)) parts.push(portsWire.connections(own.value,id,'',own.port,manage));
   const result:Wire.PortConnectionList={as_of:new Date().toISOString(),owner:'mixed',counts:portsWire.counts(parts.reduce((sum,part)=>sum+part.counts.active,0)),connections:manage?parts.flatMap(part=>part.connections??[]):null,totals:manage?parts.flatMap(part=>part.totals??[]):null,cursor:null,truncated:false};
   const inside=query.get('inside_port');if(inside!==null&&result.connections!==null) result.connections=result.connections.filter((connection)=>connection.inside.port===Number(inside));
   return {status:200,body:result};
  }
  match=/^\/nodes\/([^/]+)\/port-openings$/.exec(path);
  if(match!==null&&method==='GET') {
   const placeId=match[1]??'';const node=this.world.places.node(person,placeId);
   if(!node.access.administers) refuse(403,'authz.denied','Administration of this place is required.');
   const scene = this.scene(RUNS_ON.sandbox);
   if (scene.latency > 0) await new Promise((resolve) => setTimeout(resolve, scene.latency));
   if (scene.mode === 'outside_failed') refuse(503,'ports.unavailable','The current outside openings could not be read.');
   if (scene.mode === 'outside_refused') refuse(403,'authz.denied','Administration of this place is required.');
   const all=[...this.openings.values()].filter((opening)=>opening.desired&&opening.state!=='waiting'&&this.world.places.pathTo(this.world.resources.environments.find((environment)=>environment.id===opening.environment_id)?.home??'').some((entry)=>entry.id===placeId));
   const from=query.get('cursor')===null?0:Math.max(0,all.findIndex((opening)=>opening.id===query.get('cursor'))+1);const limit=Number(query.get('limit')??20);const shown=all.slice(from,from+limit);
   return {status:200,body:{items:shown.map((opening)=>portsWire.opening({...opening,can:{...opening.can,close:this.permissions(person,opening.environment_id).includes('ports.manage')}})),next_cursor:from+limit<all.length?shown.at(-1)?.id??null:null}};
  }
  match=/^\/hosts\/([^/]+)$/.exec(path);
  if(match!==null&&method==='GET') {
   const scene=this.scene(RUNS_ON.sandbox);if(scene.latency>0) await new Promise((resolve)=>setTimeout(resolve,scene.latency));if(scene.mode==='host_failed') refuse(503,'ports.unavailable','The host declaration could not be read.');if(scene.mode==='host_refused') refuse(403,'authz.denied','The host declaration is managed by Ada Lovelace.',{holders:[{kind:'user',user:userRef(this.world.people.get('ada')!.me)}]});
   const base=this.world.resources.handle(person,method,path,query,body);if(base===null) return null;
   return {status:200,body:{...(base.body as object),ports:scene.declaration===null?null:{incoming_address:scene.declaration.address,listen:scene.declaration.bind,ranges:scene.declaration.ranges},ports_revision:scene.declaration===null?0:1,ports_proposal:{values:{incoming_address:'host.example',listen:'all',ranges:DECLARATION.ranges},sources:{incoming_address:'Reported node address',listen:'Prepared both-family proposal',ranges:'Free sixteen-port block'},as_of:new Date().toISOString(),reason:null,limits:{incoming_address:null,listen:null,ranges:null,revision:1}}}};
  }
  match=/^\/port-openings\/([^/]+)$/.exec(path);
  if(match!==null&&(method==='GET'||method==='DELETE')) {
   const opening=this.openings.get(match[1]??'');if(opening===undefined) refuse(404,'not_found','No such opening.');this.require(person,opening.environment_id,method==='GET'?'ports.view':'ports.manage');
   if(method==='DELETE'&&opening.state!=='closed'){if(Number(query.get('expected_revision'))!==opening.revision) refuse(409,'conflict.revision','The opening changed. Refresh before closing.');opening.state='closed';opening.desired=false;opening.revision+=1;this.mutations.push({act:'close',body:{id:opening.id,expected_revision:opening.revision-1}});}
   return {status:200,body:portsWire.opening(opening)};
  }
  if (/^\/hosts\/[^/]+$/.test(path) && method === 'PATCH' && Object.hasOwn(body, 'ports')) {
   const previous = this.world.resources.handle(person, 'GET', path, query, {});
   const host = previous?.body as Wire.Host | undefined;
   if (host?.access.permissions.includes('host.manage') !== true) refuse(403, 'authz.denied', 'Host Manage is required.');
   const record = this.world.resources.hosts.find((entry) => entry.id === host.id);
   if (record === undefined) refuse(404, 'not_found', 'No such host.');
   if (body.expected_revision !== record.revision) refuse(409, 'conflict.revision', 'The host declaration changed. Review again.');
   const value = body.ports as Wire.HostPorts | null;
   if (value !== null && (!value.incoming_address || !value.listen || !Array.isArray(value.ranges) || value.ranges.length === 0)) refuse(400, 'request.invalid', 'Incoming address, Listen on and Port ranges are required.');
   record.revision += 1;
   this.scene(RUNS_ON.sandbox).declaration = value === null ? null : { bind: value.listen, address: value.incoming_address, ranges: value.ranges };
   return this.handle(person, 'GET', path, query, {});
  }
  match = /^\/environments\/([^/]+)\/ports\/(\d+)\/enter$/.exec(path);
  if (match !== null && method === 'POST') {
   const id = match[1] ?? '', number = Number(match[2]); this.require(person, id, 'ports.view');
   const row = this.port(person, id, number);
   if (!row.can.enter) refuse(403, 'ports.not_allowed', 'Current policy does not allow entry.');
   if (row.url === null) refuse(409, 'ports.domain_missing', 'No Ports domain. Use Here or the helper.');
   return { status: 200, body: { url: `${row.url}/_orch/enter?code=single-use`, expires_at: new Date(Date.now() + 30_000).toISOString() } };
  }
  return null;
 }
 control(path: string, body: Record<string, unknown>): Answer | null {
  const id = text(body.environment) || RUNS_ON.sandbox; const scene = this.scene(id);
  if (path === '/state') return { status: 200, body: { openings: [...this.openings.values()], mutations: this.mutations, requests: [...this.portApprovals.values()] } };
  if (path === '/set') {
   scene.mode = text(body.scene) || 'full'; scene.drops = typeof body.udp_dropped === 'object' && body.udp_dropped !== null ? body.udp_dropped as Wire.PortSnapshot['udp_dropped'] : undefined; scene.domain = !['no_domain', 'certificate'].includes(scene.mode); scene.declaration = scene.mode === 'no_range' ? null : structuredClone(DECLARATION); scene.latency = typeof body.latency === 'number' ? body.latency : 0; scene.lost = body.lost_link === true; scene.bindPending = body.bind_pending === true; scene.bindFailure = typeof body.bind_failure === 'string' ? body.bind_failure : null; scene.known = new Set([3000]);
   const process = { pid: 501, start: '2026-10-13T08:59:00.000Z', name: 'web-server', thread: ref(ACME.repro, 'thread', 'Repro'), item_id: null, unknown_work: false };
   scene.listeners = scene.mode === 'empty' ? [] : [ { port: 3000, protocol: 'tcp', local_address: '127.0.0.1:3000', generation: 1, identity: 'socket-web-1', first_seen_at: '2026-10-13T09:00:00.000Z', process, internal: false, unavailable_reason: null, openings: [] }, { port: 5353, protocol: 'udp', local_address: '127.0.0.1:5353', generation: 1, identity: 'socket-udp-1', first_seen_at: '2026-10-13T09:00:00.000Z', process: { ...process, name: 'datagram-service', thread: null, unknown_work: true }, internal: false, unavailable_reason: null, openings: [] }, { port: 9222, protocol: 'tcp', local_address: '127.0.0.1:9222', generation: 1, identity: 'socket-control-1', first_seen_at: '2026-10-13T09:00:00.000Z', process: { ...process, name: 'browser-control', thread: null }, internal: true, unavailable_reason: 'The browser driving endpoint stays Internal.', openings: [] } ];
   const environment = this.world.resources.environments.find((record) => record.id === id); if (environment !== undefined) environment.status = scene.mode === 'offline' ? 'stopped' : 'running';
   return { status: 204 };
  }
  if (path === '/race') { scene.race = true; return { status: 204 }; }
  if (path === '/replace-listener') { const listener = scene.listeners[0]; if (listener !== undefined) { listener.identity = randomUUID(); listener.process = listener.process === null ? null : { ...listener.process, pid: 502 }; } return { status: 204 }; }
  if (path === '/own-connections') { const value:PortConnections = { count:2,limit:CONNECTIONS_LIMIT,as_of:new Date().toISOString(),connections:['ada','grace'].map((username,index)=>({id:randomUUID(),opening_id:'',caller:{kind:'user',user:userRef(this.world.people.get(username)!.me)},outside:{address:index===0?'203.0.113.8':'203.0.113.9',port:51900+index},inside:{address:'127.0.0.1',port:48122+index},since:new Date().toISOString(),last_active_at:new Date().toISOString(),bytes_to_service:2048,bytes_from_service:8192,requests:2,state:'active'})) }; this.ownConnections.set(id,{port:3000,value}); return {status:204}; }
  if (path === '/connections') { const opening = [...this.openings.values()].find((row) => row.environment_id === id && row.way === (text(body.way) || 'host')); if (opening !== undefined) { opening.connection_count = 1; this.connections.set(opening.id, { count: 1, limit: CONNECTIONS_LIMIT, as_of: new Date().toISOString(), connections: [{ id: randomUUID(), opening_id: opening.id, caller: { kind: 'outside' }, outside: { address: '203.0.113.9', port: 51900 }, inside: { address: '127.0.0.1', port: 48122 }, since: '2026-10-13T09:00:00Z', last_active_at: new Date().toISOString(), bytes_to_service: 2048, bytes_from_service: 8192, requests: null, state: 'active' }] }); } return { status: 204 }; }
  if (path === '/request') { const kind=body.kind==='place'||body.kind==='host'?body.kind:'link';const request:Wire.PortOpeningRequest={port:3000,kind,reason:'A webhook needs to call this service.',...(kind==='link'?{until:new Date(Date.now()+3_600_000).toISOString(),entry_methods:['browser','helper'] as ('browser'|'helper')[],transports:['tcp','udp'] as Wire.PortTransport[]}:{}),...(typeof body.place_id==='string'?{place_id:body.place_id}:{})};const approval=this.seedApproval(id,request);if(body.stale===true) approval.request.plan.expires_at=new Date(Date.now()-1).toISOString();return {status:204}; }
  if (path === '/acknowledge') { for (const opening of this.openings.values()) if (opening.environment_id === id && opening.state === 'opening') { opening.state = body.failed === true ? 'failed' : 'open'; opening.host_allocated = body.failed !== true; opening.reason = body.failed === true ? 'The host could not bind the reviewed port.' : null; opening.url = body.failed === true ? null : `${scene.declaration?.address ?? 'host.example'}:${opening.host_port ?? 41000}`; opening.revision += 1; } return { status: 204 }; }
  if (path === '/lifecycle') { const outcome = text(body.outcome); for (const opening of this.openings.values()) if (opening.environment_id === id && opening.state !== 'closed' && opening.state !== 'expired') { opening.state = outcome === 'stop' ? 'stopped' : outcome === 'nothing_listens' ? 'nothing_listens' : outcome === 'remove' ? 'closed' : outcome === 'move_no_range' ? 'unavailable' : 'open'; if (outcome === 'remove') opening.desired = false; if(outcome==='stop'||outcome==='move_no_range'||outcome==='remove') opening.host_allocated=false; if (outcome === 'expiry' && opening.way === 'link') { opening.state = 'expired'; opening.desired = false; } opening.reason = outcome === 'move_no_range' ? 'The environment moved; the destination has no range. This host opening remains unavailable.' : outcome === 'preferred_changed' ? 'The automatic host address changed to host.example:41001.' : outcome === 'fixed_failed' ? 'The fixed host port could not bind.' : outcome === 'stop' ? 'The environment is stopped; the opening is kept.' : null; if (outcome === 'preferred_changed' && opening.way === 'host') { opening.host_port = 41001; opening.url = 'host.example:41001'; } if (outcome === 'fixed_failed' && opening.way === 'host') opening.state = 'failed'; opening.revision += 1; } scene.mode = outcome === 'stop' ? 'offline' : 'full'; return { status: 204 }; }
  return null;
 }
}
