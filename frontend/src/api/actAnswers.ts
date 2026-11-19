/** The rows and exact missing projections of a successful act, derived from its answer rather than broad invalidation. */
import type { Change, PathParam, Stale } from '@/api/acts';
import type { Act } from '@/api/client';
import { isCompleteThread, record, type RowChange } from '@/api/cache';
import { isDocumentDirectory, isDocumentTreeOperation, isDocumentTreeReceipt } from '@/api/documentTreeChanges';
import { AGENT_REACH, type AgentReachChange } from '@/api/agentReach';
import { META_SETTINGS } from '@/api/adminState';
import { isPeopleProjection } from '@/api/peopleChanges';

const KINDS: Readonly<Record<string, string>> = {
  nodes: 'node', orgs: 'node', projects: 'node', topics: 'node', threads: 'thread',
  providers: 'provider', agents: 'agent', environments: 'environment', hosts: 'host',
  machines: 'machine', templates: 'template', 'mcp-servers': 'mcp_server', mounts: 'mount', skills: 'skill',
  documents: 'document', tags: 'tag', groups: 'group', approvals: 'approval', jobs: 'job',
  exports: 'export', 'notification-channels': 'notification_channel',
};
const MEMBERS: Readonly<Record<string, string>> = {
  provider: 'provider', agent: 'agent', environment: 'environment', host: 'host',
  machine: 'machine', template: 'template', mcp_server: 'mcp_server', mount: 'mount', skill: 'skill',
  node: 'node', thread: 'thread', document: 'document', approval: 'approval', job: 'job',
};
const FULL_ROW_KINDS=new Set(['node','thread','document','provider','agent','environment','host','machine','template','mcp_server','mount','skill']);

function reachChanges(parts: readonly string[]): AgentReachChange[] {
  const id = parts[1];
  if (parts[0] === 'environments' && parts[2] === 'browser-profiles' && id !== undefined) return [{ agentReach: { scope: 'environment', id } }];
  const setting = parts[0] === 'admin' && parts[1] === 'settings' ? parts[2] : parts[2] === 'settings' ? parts[3] : undefined;
  if (setting !== 'env.secret_names' && setting !== 'network.allowed_hosts') return [];
  if (parts[0] === 'nodes' && id !== undefined) return [{ agentReach: { scope: 'node', id, setting } }];
  if (parts[0] === 'admin') return [{ agentReach: { scope: 'system', setting } }];
  return [];
}

function row(kind: string, value: unknown, insert = false, parent?: string): RowChange | null {
  return record(value) && typeof value.id === 'string'
    ? { row: { kind, id: value.id, value, insert, ...(parent === undefined ? {} : { parent }) } }
    : null;
}

/** Public so the registry's exhaustive test checks the policy actually used by the client. */
export function answerChanges(act: Act, p: PathParam, dependencies: readonly Change[]): Change[] {
  const related = dependencies.filter((change) => 'relatedViews' in change);
  const native = dependencies.filter((change) => !('relatedViews' in change));
  return [...nativeAnswerChanges(act, p, native), ...related];
}

function nativeAnswerChanges(act: Act, p: PathParam, dependencies: readonly Change[]): Change[] {
  if (dependencies.some((change) => 'session' in change)) return [{ session: true, ...(record(act.answer)?{mode:act.answer}:{}) }];
  // Previews, tickets and upload parts do not mutate a view, even if their answer contains identifiers.
  const parts = act.path.split('/').filter(Boolean).map((part) => decodeURIComponent(part));
  if (act.method === 'PUT' && parts[0] === 'documents' && parts[2] === 'links' && parts.length === 3 && record(act.answer)) {
    return [{ value: { key: ['document-links', parts[1], 'tabs'], data: act.answer } }];
  }
  if (parts[0] === 'me' && parts[1] === 'state' && parts[2] !== undefined && (act.input !== undefined || act.method === 'DELETE')) {
    return [{ state: { key: parts[2], data: act.answer ?? act.input ?? null, ...(parts[3] === undefined ? {} : { child: parts[3] }) } }];
  }
  // Subscription settings and bells share one native write. Its answer already includes the
  // recomputed exception page, including descendants changed by an ancestor's new default.
  if (parts[0] === 'me' && parts[1] === 'notification-exceptions' && parts[2] !== undefined && record(act.answer)
    && record(act.answer.access) && record(act.answer.exceptions) && Array.isArray(act.answer.exceptions.items)) {
    return [
      { row: { kind: 'node', id: parts[2], value: { access: act.answer.access }, partial: true } },
      { value: { key: ['subscription-choice', parts[2]], data: act.answer.access.subscription } },
      { value: { key: ['notification-exceptions', typeof act.answer.page_cursor === 'string' ? act.answer.page_cursor : null], data: act.answer.exceptions } },
    ];
  }
  if (act.path === '/document-tree/operations' && isDocumentTreeReceipt(act.answer) && isDocumentTreeOperation(act.input)) {
    return [{ documentTree: { receipt: act.answer, input: act.input } }];
  }
  if (act.method === 'POST' && (parts[0] === 'document-containers' || parts[0] === 'nodes') && parts[2] === 'documents'
    && parts.length === 3 && record(act.answer) && typeof act.answer.id === 'string' && record(act.answer.location)
    && isDocumentDirectory(act.answer.location.parent)) {
    return [{ row: { kind: 'document', id: act.answer.id, value: act.answer, insert: true } },
      { documentDirectories: [act.answer.location.parent] }];
  }
  if (act.method === 'DELETE' && parts[0] === 'invitations' && parts.length === 2 && parts[1] !== undefined) {
    // Revocation keeps this invitation in native history. Its 204 has no replacement row;
    // select the exact list already holding it, not a query keyed by the invitation ID.
    return [{ missing: [{ key: ['invitations'] }], containingRow: parts[1], cancelPending: true }];
  }
  if (dependencies.length === 0) return [];
  const answer = act.answer;
  const input = act.input;
  const reach = reachChanges(parts);
  // Enrollment creates a one-time token, not a machine/node row with the token's identifier.
  if (parts.at(-1) === 'enrollment-tokens') return [];
  const changes: Change[] = [];
  const put = (kind: string, value: unknown, insert = false, parent?: string): void => {
    if ((kind === 'thread' || kind === 'node') && isCompleteThread(value)) {
      changes.push({ thread: value });
      return;
    }
    // A returned native row can also enter a filter it now matches after a rename/state change.
    const found = row(kind, value, insert || record(value) && (typeof value.name === 'string' || kind === 'document' && typeof value.title === 'string' && record(value.access)), parent);
    if (found !== null) {
      if(kind === 'thread' || FULL_ROW_KINDS.has(kind) && !record(found.row.value?.access)) {
        found.row.partial=true;found.row.insert=false;
        changes.push(found,{missing:[{key:[kind==='mcp_server'?'mcp-server':kind,found.row.id],exact:true}]});
      } else changes.push(found);
    }
  };
  const value = (key: readonly unknown[], data: unknown): void => { changes.push({ value: { key, data } }); };
  if (act.method==='POST' && parts[0]==='threads' && parts[2]==='calls' && parts[3]==='recovery' && record(answer) && 'recovery' in answer) {
    return [{value:{key:['call-recovery',p('id')],data:answer}},...dependencies.filter((change)=>'feed' in change)];
  }
  const meta = (): Change => ({ meta: true, ...(act.requestId === undefined ? {} : { requestId: act.requestId }) });
  // Assigning tags returns the owning place. Defining a tag uses POST and returns a tag instead.
  if (act.method === 'PUT' && parts[0] === 'nodes' && parts[2] === 'tags' && parts.length === 3) {
    if (record(answer) && answer.id === parts[1]) put('node', answer);
    return changes;
  }
  if (act.method === 'POST' && parts[0] === 'notification-channels' && parts[1] !== undefined
    && parts[2] === 'test' && record(answer) && typeof answer.ok === 'boolean') {
    // A test receipt omits the stored delivery time and error. Read only its loaded owners.
    const channelId = parts[1];
    return ['channels', 'notification-channels'].map((kind): Change => ({
      missing: [{ key: [kind] }], containingRow: channelId, cancelPending: true,
    }));
  }
  if ((act.method === 'POST' && parts[0] === 'nodes' && parts[2] === 'destinations' && parts.length === 3
    || act.method === 'PATCH' && parts[0] === 'destinations' && parts.length === 2)
    && record(answer) && typeof answer.id === 'string' && typeof answer.channel === 'string'
    && record(answer.node) && typeof answer.node.id === 'string') {
    return [{ collection: { key: ['destinations', answer.node.id], identity: 'id', id: answer.id, data: answer }, cancelPending: true }];
  }
  if (parts[0] === 'destinations' && parts[1] !== undefined
    && (act.method === 'DELETE' && parts.length === 2 || act.method === 'POST' && parts[2] === 'test')) {
    // These receipts omit the stored list state; the loaded row identifies its exact owner.
    return [{ missing: [{ key: ['destinations'] }], containingRow: parts[1], cancelPending: true }];
  }
  if (act.method === 'DELETE' && parts[0] === 'service-accounts' && parts[1] !== undefined && parts.length === 2) {
    return [{ missing: [{ key: ['service-accounts'] }], containingAccount: parts[1], cancelPending: true }];
  }
  if (parts[0] === 'service-accounts' && parts[1] !== undefined && parts[2] === 'tokens'
    && (act.method === 'POST' && parts.length === 3 || act.method === 'DELETE' && parts.length === 4)) {
    const id = act.method === 'POST' && record(answer) && record(answer.info) && typeof answer.info.id === 'string' ? answer.info.id : parts[3];
    if (id !== undefined) changes.push({ collection: { key: ['service-tokens', parts[1]], identity: 'id', id, data: act.method === 'DELETE' ? null : record(answer) ? answer.info : null } });
    // TokenCreated omits the independently projected account count. Retake its shown owner once;
    // another org and an unopened account list cannot be read by this act.
    changes.push({ missing: [{ key: ['service-accounts'] }], containingAccount: parts[1], cancelPending: true });
    return changes;
  }
  if (parts[0] === 'hosts' && parts[1] !== undefined && parts[2] === 'runtimes' && parts[3] !== undefined) {
    const key = ['host-runtimes', parts[1]];
    if (act.method === 'POST' && parts[4] === 'stop' && record(answer) && answer.id === parts[3]) {
      return [{ collection: { key, identity: 'id', id: parts[3], data: answer }, cancelPending: true }];
    }
    // Removal may be scheduled. Its acknowledgement cannot say which rows remain on the host.
    if (act.method === 'DELETE' && parts.length === 4) return [{ missing: [{ key, exact: true }], cancelPending: true }];
  }
  if (parts[0] === 'usage' && parts[1] === 'caps') {
    const id = record(answer) && typeof answer.id === 'string' ? answer.id : parts[2];
    if (id !== undefined) return [{ capViews: { id, ...(record(answer) ? { value: answer } : {}) } }];
  }
  if ((parts[0] === 'nodes' || parts[0] === 'environments') && parts[1] !== undefined && parts[2] === 'secrets' && parts[3] !== undefined) {
    const key = ['secrets', parts[1]];
    if (act.method === 'PUT' && record(answer) && answer.name === parts[3]) {
      return [{ collection: { key, identity: 'name', id: parts[3], data: answer }, cancelPending: true }];
    }
    // Removing an override can expose the same name from a parent; only this list's reader knows it.
    if (act.method === 'DELETE') return [{ missing: [{ key, exact: true }], cancelPending: true }];
  }
  if (parts[0] === 'admin' && parts[1] === 'users') {
    const user = parts.length === 2 && record(answer) ? answer.user : parts.length === 3 ? answer : undefined;
    if (record(user) && typeof user.id === 'string' && typeof user.username === 'string' && typeof user.display_name === 'string'
      && (user.status === 'active' || user.status === 'suspended') && typeof user.is_system_admin === 'boolean') {
      return [{ adminUser: { id: user.id, value: user } },
        { row: { kind: 'user', id: user.id, value: { id: user.id, kind: user.kind, username: user.username, display_name: user.display_name, avatar: user.avatar }, partial: true } }];
    }
    if (parts[2] !== undefined && parts[3] === 'org-invitation') return [{ adminUser: { id: parts[2] } }];
  }
  if (parts[0] === 'admin' && parts[1] === 'identity-providers' && parts.length <= 3) {
    const id = record(answer) && typeof answer.id === 'string' ? answer.id : parts[2];
    if (id !== undefined && (record(answer) || act.method === 'DELETE')) return [
      { collection: { key: ['admin', 'identity-providers'], identity: 'id', id, data: act.method === 'DELETE' ? null : answer } }, meta(),
    ];
  }
  if (parts[0] === 'admin' && parts[1] === 'harnesses' && act.method === 'PUT' && Array.isArray(answer)) {
    return [{ value: { key: ['admin', 'harnesses'], data: answer } }, { missing: [{ key: ['harness-kinds'], exact: true }], cancelPending: true }];
  }
  if (act.path === '/admin/usage/catalogue' && record(answer) && Array.isArray(answer.families)) {
    return [{ value: { key: ['usage', 'catalogue', null], data: answer } }, { rateViews: { nodeId: null } }];
  }
  if (act.path === '/me/sessions' && act.method === 'DELETE' && Array.isArray(answer)) return [{ value: { key: ['sessions'], data: answer } }];
  if (parts[0] === 'me' && parts[1] === 'sessions' && parts[2] !== undefined && act.method === 'DELETE') {
    return [{ collection: { key: ['sessions'], identity: 'id', id: parts[2], data: null }, cancelPending: true }];
  }
  // One-time codes are handed straight to the dialog; only the safe account/metadata enter caches.
  if (act.path === '/me/totp/enable' && record(answer) && record(answer.me) && record(answer.recovery_codes)) {
    value(['me'], answer.me);
    if (record(answer.recovery_codes.status)) value(['recovery-codes'], answer.recovery_codes.status);
    return changes;
  }
  if (act.path === '/me/recovery-codes' && record(answer) && record(answer.status)) return [{ value: { key: ['recovery-codes'], data: answer.status } }];
  if (act.path === '/me/second-step/verify' && record(answer)) return [{ value: { key: ['me'], data: answer } }];
  if(parts[0]==='me' && parts[1]==='tokens') {
    if(record(answer) && record(answer.info)) put('api_token',answer.info,true);
    else if(act.method==='DELETE' && parts[2]!==undefined) changes.push({row:{kind:'api_token',id:parts[2],value:null}});
    if(changes.length>0) return changes;
  }
  if(parts[0]==='environments' && (parts[2]==='browsers' || parts[2]==='displays' || parts[2]==='browser-profiles')) {
    const field=parts[2]==='browser-profiles' ? 'profiles' : parts[2];
    const key=[field==='displays'?'environment-displays':'environment-browsers',parts[1]];
    const identity=field==='profiles' ? 'name' : 'id';
    const id=record(answer) && typeof answer[identity]==='string' ? answer[identity] : parts[3];
    if(typeof id==='string' && (act.method==='DELETE' || record(answer))) return [{collection:{key,field,identity,id:act.method==='PATCH' && parts[3]!==undefined ? parts[3] : id,data:act.method==='DELETE'?null:answer}}, ...reach];
    if(reach.length>0) return reach;
  }
  if((parts[0]==='browsers' || parts[0]==='displays') && act.method==='DELETE' && parts[1]!==undefined) return [{row:{kind:parts[0]==='browsers'?'browser':'display',id:parts[1],value:record(answer)?answer:null}}];
  if (act.method === 'PUT' && parts[0] === 'me' && parts[1] === 'first-steps' && record(answer)) return [{ value: { key: ['me-state'], data: answer } }];
  if (parts[2] === 'permissions' && record(answer) && Array.isArray(answer.rows)) return [{ value: { key: ['page', parts[0], parts[1]], data: answer } }];
  if (parts[0] === 'nodes' && parts[2] === 'defaults' && record(answer) && Array.isArray(answer.rows)) return [{ value: { key: ['defaults', p('id')], data: answer } }];
  if (act.path === '/admin/defaults' && record(answer) && Array.isArray(answer.rows)) return [{ value: { key: ['defaults', 'system'], data: answer } }];
  if (parts.includes('notification-channels') && record(answer) && typeof answer.id === 'string' && typeof answer.kind === 'string') {
    const owner = answer.owner;
    const scope = owner === 'system' ? 'system' : record(owner) && record(owner.org) && typeof owner.org.id === 'string' ? owner.org.id : undefined;
    if (scope !== undefined) changes.push({ collection: { key: ['channels', scope], identity: 'id', id: answer.id, data: answer } });
  }
  if (parts[0] === 'environments' && parts[2] === 'files' && record(answer) && typeof answer.path === 'string') return [{ value: { key: ['workspace-stat', p('id'), answer.path], data: answer } }];
  if ((parts[2] === 'settings' || parts[0] === 'admin' && parts[1] === 'settings') && record(answer) && typeof answer.key === 'string') {
    const key = parts[0] === 'nodes' ? ['settings', p('id')] : parts[0] === 'admin' ? ['admin', 'settings'] : [`${KINDS[parts[0] ?? ''] ?? ''}-settings`, p('id')];
    const rates: Change[] = (parts[0] === 'nodes' || parts[0] === 'admin') && ['usage.rates', 'usage.catalogue', 'usage.catalogue_scale'].includes(answer.key)
      ? [{ rateViews: { nodeId: parts[0] === 'nodes' ? p('id') : null } }] : [];
    return [{ collection: { key, identity: 'key', id: answer.key, data: answer } }, ...reach,
      ...rates,
      ...(parts[0] === 'admin' && META_SETTINGS.has(answer.key) ? [meta()] : [])];
  }
  if (parts[0] === 'providers' && parts[2] === 'models') {
    if(parts[3]==='suggestions' && parts[4]==='accept' && record(answer) && record(answer.provider)) {
      put('provider',answer.provider);
      if(Array.isArray(answer.provider.models))value(['provider-models',p('id')],answer.provider.models);
      return changes;
    }
    const modelChanges=(id:string,data:unknown):Change[]=>[
      {collection:{key:['provider-models',p('id')],identity:'id',id,data}},
      {collection:{key:['provider',p('id')],field:'models',identity:'id',id,data}},
    ];
    if (Array.isArray(answer)) return answer.flatMap((model: unknown) => record(model) && typeof model.id === 'string'
      ? modelChanges(model.id,model) : []);
    const id = record(answer) && typeof answer.id === 'string' ? answer.id : p('model_id');
    if (id !== '') return modelChanges(id,act.method === 'DELETE' ? null : answer);
  }
  if (parts[0] === 'machines' && parts[2] === 'policy' && record(answer)) {
    value(['machine-policy',p('id')],answer);
    if(record(answer.machine_view)) put('machine',answer.machine_view);
    return changes;
  }
  if(parts[0]==='machines' && parts[2]==='roles' && record(answer)) {
    // The command acknowledges a saved selection. Its active-role read can predate a newer hello;
    // the live machine projection is the sole source of active roles after the reconnect.
    return changes;
  }
  if(parts[0]==='machines' && parts[2]==='programs' && record(answer)) {
    value(['machine-programs',p('id')],answer);
    if(record(answer.machine_view))put('machine',answer.machine_view);
    return changes;
  }

  if (record(answer)) {
    // Composite replies (a provider plus its job, an archive request, a fork) carry each row once.
    for (const [member, kind] of Object.entries(MEMBERS)) if (member in answer) {
      const value = answer[member];
      // Approval.node and similar members are relationships, not changed entity views. In
      // particular, a thread's NodeRef must never replace its cached Thread or ask it again.
      if (FULL_ROW_KINDS.has(kind) && (!record(value) || !record(value.access))) continue;
      if ((kind === 'thread' || kind === 'node' && record(value) && value.kind === 'thread') && !isCompleteThread(value)) continue;
      put(kind, value, act.method === 'POST' && record(value) && value.id !== p('id'));
    }
  }
  if (act.path === '/me' && record(answer)) {
    value(['me'], answer);
    // The same person's name/avatar can already be in the bootstrap and resource-owner rows.
    // Overlay existing public reference fields without widening them into a private Me DTO.
    if(typeof answer.id==='string') changes.push({row:{kind:'user',id:answer.id,value:answer,partial:true}});
  }
  else if (act.path === '/me/notification-prefs' && record(answer)) value(['notification-prefs'], answer);
  else if (parts[0] === 'me' && parts[1] === 'state' && parts[2] !== undefined) {
    // These writes are normally optimistic; the answer is authoritative when it carries the value.
    return answer === undefined && input === undefined ? [] : [{ state: { key: parts[2], data: answer === undefined ? input : answer } }];
  } else if (parts[0] === 'providers' && parts[2] === 'login' && record(answer)) {
    value(['provider-login', parts[1]], answer);
  } else if (parts[0] === 'forks' && record(answer)) {
    value(['fork-batch', typeof answer.id === 'string' ? answer.id : p('batch_id')], answer);
  } else {
    // The rightmost collection is the row's kind; a model is not its provider, nor a child its parent.
    let collection = -1;
    for (let index = 0; index < parts.length; index += 1) if (KINDS[parts[index] ?? ''] !== undefined) collection = index;
    const kind = KINDS[parts[collection] ?? ''];
    const id = parts[collection + 1];
    const child = parts[collection + 2];
    if (kind !== undefined && record(answer) && typeof answer.id === 'string') {
      // A Job returned by an environment action describes the job, not the environment.
      const isJob = typeof answer.state === 'string' && ('progress' in answer) && ('batch_id' in answer || 'result' in answer);
      const native = isJob ? 'job' : kind;
      const nestedModel = parts.includes('models') || parts.includes('runtimes') || parts.includes('permissions');
      const named = typeof answer.name === 'string' || native === 'document' && typeof answer.title === 'string' || native==='approval' && typeof answer.state==='string';
      const belongs=child===undefined || answer.id===id || child==='copy' || child==='fork';
      // A job has its own identity even when a nested environment action created it.
      if (isJob || (!nestedModel && belongs && named)) put(native, answer, act.method === 'POST' && (id === undefined || answer.id !== id), collection > 0 ? parts[collection - 1] : undefined);
    } else if (kind !== undefined && act.method === 'DELETE' && id !== undefined && child === undefined) {
      changes.push({ row: { kind, id, value: null } });
    }
  }
  if (act.method === 'POST' && parts[0] === 'nodes' && parts[2] === 'restore' && parts.length === 3 && record(answer) && answer.kind === 'org' && answer.parent_id === null && answer.org_kind === 'team') {
    // The returned Org does not contain the restored subtree's paged list membership.
    changes.push({ adminOrgRestore: true });
  }
  if (act.method === 'POST' && act.path === '/admin/agents' && record(answer) && answer.home === null && record(answer.access)) {
    // The returned row does not determine the bounded /agents list's membership/order.
    changes.push({ missing: [{ key: ['admin', 'agents'], exact: true }], cancelPending: true });
  }
  if (changes.length > 0) return changes;

  // Membership answers describe the accepted destination or member, not the
  // other visible rosters, counts and invitation uses that changed with it.
  // Keep their explicit repair owner, without rereading its projections through
  // the legacy hints. Other missing views retain their existing owner below.
  const people = dependencies.filter((change) => 'people' in change);

  // A command whose answer lacks its resulting view may read that ONE exact observed projection.
  // Lists and broad kinds are always maintained by their scoped feeds, never asked again here.
  const id=p('id') || p('node_id') || p('org_id') || p('environment_id') || p('provider_id') || p('thread_id');
  const projections=dependencies.filter((change):change is Stale=>'key' in change && change.gone!==true && !(reach.length>0 && change.key[0]===AGENT_REACH)
    && !(people.length>0 && isPeopleProjection(change.key)))
    .map((change)=>id!=='' && change.id===undefined && change.exact!==true ? {...change,id} : change);
  return [...reach, ...people, ...(projections.length===0 ? [] : [{missing:projections}])];
}
