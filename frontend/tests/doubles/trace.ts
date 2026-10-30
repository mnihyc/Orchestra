/** The permanent scoped trail and lifecycle facts of the shared fake world. */
import { randomUUID } from 'node:crypto';
import type { DocumentOpening } from './admin-openings.ts';
import { ACTIONS, kindOf } from '../../src/features/trace/catalogue.ts';
import type { DocumentSync, HostKeptData, Job, Node, NodeRef, ThreadSync, TreeNode, DeletedEnvironment, EnvironmentRemoval, DeletedInspection as Inspection, Environment as RetainedEnvironment, AuditEntry as TraceEntry, AuditEntryPage as TracePage } from '../../src/generated/api-types.ts';
import { ACME, id, RUNS_ON, TRACE, userRef } from './fixtures.ts';
import { sceneOf } from './thread-scenes.ts';
import type { PlanSeed } from './plan-usage.ts';
import { type Answer, type Person, Refusal, type World } from './world.ts';

/** More than two normal pages makes continuation observable without a large browser fixture. */
const TRAIL_ROWS = 85;
const EVENT_DAY = '2026-10-12T12:00:00.000Z';
const PAGE_DEFAULT = 40;
const DAY_MS = 24 * 60 * 60 * 1000;
const EXPORT_DAYS = 366;
/** The shipped window covers a week; a test may set the resolved policy before scheduling. */
const DEFAULT_KEEP_DAYS = 7;
const KEPT_BYTES = 2 * 1024 ** 3;
const lifecycle = { name: null, state: 'live' as const, deletion_id: null, deleted_at: null, deleted_by: null, purge_at: null, purged_at: null, can_open: false, can_inspect: false, can_restore: false };
function declaredKind(action: string): NonNullable<TraceEntry['kind']> { const found = kindOf(action); if (found === undefined) throw new Error('A fixture action must have a declared kind: ' + action); return found; }
interface Stored { entry: TraceEntry; ancestry: string[] }
interface Retained { kind: string; name: string; home: string; state: 'deleted' | 'purged'; inspection: Inspection; node?: Node; thread?: ThreadSync; document?: DocumentSync }
export class TraceWorld {
  private readonly privateOpenings = new Set<string>();
  world: World;
  records: Stored[] = [];
  requests: { method: string; path: string; query: Record<string, string>; body: Record<string, unknown>; idempotency_key: string | null }[] = [];
  delayMs = 0;
  failing = false;
  incomplete = false;
  retained = new Map<string, Retained>();
  frames: Record<string, unknown>[] = [];
  inspections = new Set<string>();
  removals = new Map<string, EnvironmentRemoval>();
  jobs = new Map<string, Job>();
  polls = new Map<string, number>();
  deletionIds = new Map<string, string>();
  retainedArchive = new Map<string, Retained>();
  deletionSequence = 500;
  completedActs = new Map<string, Answer>();
  keepDays = DEFAULT_KEEP_DAYS;
  retentionFailure: string | null = null;
  holdJobs = false;
  barred = new Set<string>();
  constructor(world: World) { this.world = world; }
  /** The external reset markers in the plan scenario open matching rows in the ordinary trail. */
  seedPlanResets(provider:{id:string;name:string;home:string},seed:PlanSeed):void {
    const home=this.world.places.rowOf(provider.home);
    if(home===undefined)throw new Error('A seeded provider needs its home');
    const node:NodeRef={id:home.id,kind:home.kind,name:home.name,redacted:false};
    const resets=seed.rows.flatMap((row)=>row.resets.filter((reset)=>reset.cause!=='orch').map((reset)=>({window:row.window_id,reset})));
    const ids=new Set(resets.map(({reset})=>reset.id));
    this.records=this.records.filter(({entry})=>!ids.has(entry.id));
    for(const {window,reset} of resets) {
      const facts=seed.facts.windows.find((item)=>item.id===window);
      if(facts===undefined)throw new Error('A seeded reset needs its window');
      this.records.unshift({ancestry:this.world.places.pathTo(home.id).map((place)=>place.id),entry:{
        id:reset.id,at:reset.at,actor:{kind:'system'},action:'provider.window_started_over',kind:declaredKind('provider.window_started_over'),
        object:{...lifecycle,id:provider.id,kind:'provider',name:provider.name,can_open:true},node,
        as_admin:false,request_id:null,event_deletion_id:null,history:true,override:false,ip:null,
        summary:reset.cause==='outside' ? 'A reset was spent outside Orch' : 'The provider started a usage window over early',
        details:{window_id:window,window_label:facts.label,before_percent:80,after_percent:facts.used_percent,previous_resets_at:facts.resets_at,cause:reset.cause},
      }});
    }
  }
  /** The native logical-opening key; the caller supplies only freshly authorized facts. */
  openedDocument(opening: DocumentOpening): boolean {
    const { person, session, nonce, document, requestId } = opening;
    const key = JSON.stringify([person.me.id, session, 'document', document.id, nonce]);
    if (this.privateOpenings.has(key)) return false;
    this.records.unshift({ ancestry: [], entry: {
      id: randomUUID(), at: new Date().toISOString(), actor: { kind: 'user', user: userRef(person.me) },
      action: 'admin.opened', kind: declaredKind('admin.opened'), as_admin: true, history: false, override: false,
      details: { place: 'system', session_id: session, opening_id: nonce }, node: null,
      object: { ...lifecycle, kind: 'document', id: document.id, name: document.name, can_open: true },
      request_id: requestId, event_deletion_id: null, ip: null,
      summary: 'opened a thing through administrative access',
    } });
    this.privateOpenings.add(key);
    return true;
  }
  seed(): void {
    const actor = this.world.places.userOfName('dora');
    const acts = ['node.created', 'document.renamed', 'node.deleted', 'permission.removed', 'turn.interrupted', 'auth.login'] as const;
    for (let index = 0; index < TRAIL_ROWS; index += 1) {
      const action = acts[index % acts.length] ?? 'node.created';
      const kind = declaredKind(action);
      const nodeId = index % 3 === 0 ? ACME.baselineThread : ACME.mainThread;
      const row = this.world.places.rowOf(nodeId);
      const node: NodeRef = { id: nodeId, kind: 'thread', name: row?.name ?? null, redacted: false };
      this.records.push({ ancestry: this.world.places.pathTo(nodeId).map((place) => place.id), entry: { as_admin: false, request_id: null, event_deletion_id: null, id: id(26, index + 1), at: new Date(Date.parse(EVENT_DAY) - index * 60_000).toISOString(), actor: { kind: 'user', user: actor }, action, kind, object: { ...lifecycle, kind: 'thread', id: nodeId, name: node.name, state: 'live', deleted_at: null, deleted_by: null, purge_at: null, purged_at: null, can_open: true, can_inspect: false, can_restore: false }, node, history: false, override: false, summary: `${actor.display_name} · ${action} · ${node.name}`, details: { provider: this.world.resources.bindingOf(nodeId).provider, model: 'team-model', environment: this.world.resources.bindingOf(nodeId).environment }, ip: null } });
    }
    this.seedRetained();
    this.seedEnvironments();
    for (const key of [...this.retained.keys(), ...this.removals.keys()]) this.deletionIds.set(key, key === TRACE.child ? this.deletionIds.get(TRACE.project)! : id(31, this.deletionIds.size + 1));
    for (const [key, held] of this.retained) this.retainedArchive.set(key, structuredClone(held));
    for (const { entry } of this.records) {
      entry.request_id = null; entry.event_deletion_id = null;
      if (entry.object.id !== null && this.deletionIds.has(entry.object.id)) { entry.event_deletion_id = this.deletionIds.get(entry.object.id)!; entry.object.deletion_id = entry.event_deletion_id; }
    }
    this.records.unshift({ ancestry: [ACME.org, ACME.web], entry: { as_admin: false, request_id: null, event_deletion_id: null, id: id(26, 901), at: '2026-10-12T12:05:00.000Z', actor: { kind: 'user', user: this.world.places.userOfName('ada') }, action: 'permission.added', kind: 'access', object: { ...lifecycle, kind: 'project', id: ACME.web, name: 'Web', state: 'live', can_open: true, can_inspect: false, can_restore: false }, node: { id: ACME.web, kind: 'project', name: 'Web', redacted: false }, summary: 'Ada Lovelace named herself on the page of Web', details: { override: true }, history: true, override: true, ip: null } });
  }
  private seedEnvironments(): void {
    const template = this.world.resources.environments.find((entry) => entry.id === RUNS_ON.sandbox);
    if (template === undefined) return;
    for (const [key, name, bytes] of [[TRACE.environment, 'Kept workspace', KEPT_BYTES], [TRACE.unknownEnvironment, 'Unmeasured cache', null]] as const) {
      this.world.resources.environments.push({ ...structuredClone(template), id: key, name, home: ACME.web, owner: 'dora', status: 'removed' });
      this.removals.set(key, { phase: 'kept', deleted_at: EVENT_DAY, deleted_by: this.world.places.userOfName('dora'), keep_days: DEFAULT_KEEP_DAYS, purge_at: '2026-10-19T12:00:00.000Z', purged_at: null, bytes, size_at: bytes === null ? null : EVENT_DAY, reason: bytes === null ? 'The host has not reported a measured size.' : null, revision: 1, can_inspect: true, can_restore: true, can_purge: false, job: null });
      this.records.unshift({ ancestry: [...this.world.places.pathTo(ACME.web).map((place) => place.id), key], entry: { as_admin: false, request_id: null, event_deletion_id: null, id: id(28, this.records.length + 1), at: EVENT_DAY, actor: { kind: 'user', user: this.world.places.userOfName('dora') }, action: 'environment.deleted', kind: 'deleted', object: { ...lifecycle, id: key, kind: 'environment', name, state: 'deleted', deleted_at: EVENT_DAY, deleted_by: this.world.places.userOfName('dora'), purge_at: '2026-10-19T12:00:00.000Z', purged_at: null, can_open: false, can_inspect: true, can_restore: true }, node: { id: ACME.web, kind: 'project', name: 'Web', redacted: false }, history: true, override: false, summary: `Dora Developer deleted ${name}`, details: { version: 1, name, bytes }, ip: null } });
    }
  }
  private environment(person: Person, key: string): RetainedEnvironment {
    const answer = this.world.resources.handle(person, 'GET', `/environments/${key}`, new URLSearchParams(), {});
    if (answer === null) throw new Refusal(404, 'not_found', 'no such environment');
    const env = answer.body as RetainedEnvironment; const home = this.world.places.rowOf(env.home.id);
    const visible = home !== undefined && this.world.places.access(person, home.id).permissions.includes(`${home.kind}.view`);
    return home === undefined ? env : { ...env, home: { id: home.id, kind: home.kind, name: visible ? home.name : null, redacted: !visible } };
  }
  private environmentRemoval(person: Person, key: string): EnvironmentRemoval | null {
    const facts = this.removals.get(key); if (facts === undefined) return null;
    const env = this.environment(person, key); const admin = this.world.places.access(person, env.home.id).administers;
    const manager = env.access.permissions.includes('environment.manage'); const inspect = (manager || admin) && !this.barred.has(`${person.me.username}:inspect`);
    const kept = facts.phase === 'kept';
    return { ...facts, can_inspect: inspect && facts.phase !== 'purged', can_restore: inspect && kept && !this.barred.has(`${person.me.username}:restore`), can_purge: admin && kept && !this.barred.has(`${person.me.username}:purge`) };
  }
  /** Only the ordinary host reader calls this, after its HostManage cut, as the server does. */
  hostKeptData(person: Person, hostId: string): HostKeptData[] {
    return [...this.removals].flatMap(([key, facts]) => {
      if (!['kept', 'restoring', 'purging'].includes(facts.phase)) return [];
      const env = this.environment(person, key);
      if (env.host?.id !== hostId) return [];
      const removal = this.environmentRemoval(person, key);
      if (removal === null) return [];
      return [{ environment_id: key, name: removal.can_inspect ? env.name : null, home: env.home,
        storages: [env.workspace_storage], phase: removal.phase, bytes: removal.bytes,
        purge_at: removal.purge_at, job: removal.job }];
    });
  }
  private deletedEnvironment(person: Person, key: string, hostOnly = false): DeletedEnvironment {
    const env = this.environment(person, key); const removal = this.environmentRemoval(person, key);
    if (removal === null) throw new Refusal(404, 'not_found', 'no removal record');
    return { environment: { id: env.id, name: env.name, address: env.address, owner: env.owner, placement: env.placement, status: env.status, ports_exposed: env.ports_exposed }, home: env.home, host: env.host, storage: env.workspace_storage, removal: hostOnly ? { ...removal, can_inspect: false, can_restore: false, can_purge: false } : removal };
  }
  private beginJob(person: Person, key: string, kind: 'environment_restore' | 'environment_purge'): Job {
    const facts = this.removals.get(key); if (facts === undefined || facts.phase === 'purged') throw new Refusal(410, 'gone', 'the kept files are gone');
    const job: Job = { id: id(29, this.jobs.size + 1), kind, state: 'queued', environment_id: key, node_id: this.environment(person, key).home.id, provider_id: null, created_at: new Date().toISOString(), created_by: userRef(person.me), started_at: null, finished_at: null, error: null, log: null, result: null, batch_id: null, progress: { bytes_done: 0, bytes_total: facts.bytes, eta_seconds: null, files_done: 0, files_total: null, message: null, rate_bps: null, phases: [], percent: 0, phase: 'Waiting for the kept files' } };
    facts.phase = kind === 'environment_restore' ? 'restoring' : 'purging'; facts.revision += 1; facts.job = { id: job.id, kind, state: job.state, phase: job.progress.phase, percent: job.progress.percent };
    this.jobs.set(job.id, job); this.polls.set(job.id, 0); return job;
  }
  private advance(job: Job): void {
    if (!['queued', 'running'].includes(job.state) || this.holdJobs) return;
    const key = job.environment_id ?? ''; const facts = this.removals.get(key); if (facts === undefined) return;
    const count = (this.polls.get(job.id) ?? 0) + 1; this.polls.set(job.id, count); job.state = 'running'; job.started_at ??= new Date().toISOString();
    job.progress.percent = count === 1 ? 35 : count === 2 ? 70 : 100;
    job.progress.phase = count === 1 ? 'Checking kept volumes' : count === 2 ? (job.kind === 'environment_restore' ? 'Verifying the stopped unit' : 'Removing kept volumes') : 'Committed';
    if (count >= 3) {
      job.finished_at = new Date().toISOString();
      if (this.retentionFailure !== null) {
        job.state = 'failed'; job.error = { code: 'environment.unavailable', message: this.retentionFailure, phase: job.progress.phase, undone: true }; facts.phase = 'kept'; facts.reason = this.retentionFailure; facts.revision += 1;
        this.world.notifications.add('ada', { kind: 'environment.attention', title: 'Kept files need attention', body: this.retentionFailure, node: this.environment(this.world.people.get('ada')!, key).home, url: `/resources/hosts/${this.environment(this.world.people.get('ada')!, key).host?.id ?? ''}`, data: { environment_id: key, job_id: job.id } });
      } else {
        job.state = 'succeeded'; const record = this.world.resources.environments.find((entry) => entry.id === key);
        if (job.kind === 'environment_restore') { if (record !== undefined) { record.status = 'stopped'; record.generation = (record.generation ?? 0) + 1; record.revision += 1; } this.removals.delete(key); this.deletionIds.delete(key); job.result = { environment_id: key, status: 'stopped' }; }
        else { facts.phase = 'purged'; facts.purged_at = job.finished_at; facts.revision += 1; facts.can_inspect = false; facts.can_restore = false; facts.can_purge = false; job.result = { environment_id: key, purged_at: facts.purged_at }; }
      }
    }
    if (this.removals.has(key)) facts.job = { id: job.id, kind: job.kind, state: job.state, phase: job.progress.phase, percent: job.progress.percent };
  }
  private retentionHandle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    let found: RegExpExecArray | null;
    if ((found = /^\/environments\/([^/]+)$/.exec(path))) {
      const key = found[1] ?? ''; const env = this.environment(person, key); const facts = this.environmentRemoval(person, key);
      if (method === 'GET') {
        if (query.get('inspect_deleted') === 'true') {
          if (facts === null || facts.phase === 'purged') throw new Refusal(410, 'gone', 'there are no kept files');
          if (facts.deleted_at === null || facts.purge_at === null) throw new Refusal(404, 'not_found', 'the unit has not been removed');
          if (!facts.can_inspect) throw new Refusal(403, 'authz.denied', 'environment restoration authority required', { required: 'environment.manage', missing: [] });
          return { status: 200, body: { ...env, removal: facts, inspection: { deleted_at: facts.deleted_at, deleted_by: facts.deleted_by, purge_at: facts.purge_at, readonly: true } } satisfies RetainedEnvironment };
        }
        if (env.status === 'removed') throw new Refusal(404, 'not_found', 'the environment was removed');
        return { status: 200, body: { ...env, removal: facts } };
      }
      if (method === 'DELETE') {
        if (!env.access.permissions.includes('environment.manage')) throw new Refusal(403, 'authz.denied', 'environment.manage required');
        if (query.get('confirm') !== env.name) throw new Refusal(400, 'request.invalid', 'name the environment');
        if (env.threads.length > 0) throw new Refusal(409, 'resource.in_use', 'active threads are still bound');
        const at = query.get('at') ?? new Date().toISOString();
        const keepDays = env.temporary === null || env.temporary === undefined ? this.keepDays : 0;
        const purgeAt = new Date(Date.parse(at) + keepDays * DAY_MS).toISOString();
        const removal: EnvironmentRemoval = { phase: 'scheduled', deleted_at: null, deleted_by: userRef(person.me), keep_days: keepDays, purge_at: null, purged_at: null, bytes: null, size_at: null, reason: null, revision: 1, can_inspect: false, can_restore: false, can_purge: false, job: null }; this.removals.set(key, removal);
        const record = this.world.resources.environments.find((entry) => entry.id === key); if (record !== undefined) record.status = 'removed';
        removal.phase = 'kept'; removal.deleted_at = at; removal.purge_at = purgeAt;
        return { status: 202, body: { ...env, status: 'removed', remove_at: null, removal } };
      }
    }
    if ((found = /^\/nodes\/([^/]+)\/environments\/deleted$/.exec(path)) && method === 'GET') {
      const scope = found[1] ?? ''; this.world.places.node(person, scope);
      const rows = [...this.removals].filter(([key, facts]) => facts.phase !== 'purged' && ['kept', 'restoring', 'purging'].includes(facts.phase) && this.world.places.pathTo(this.environment(person, key).home.id).some((place) => place.id === scope)).flatMap(([key]) => { const removal = this.environmentRemoval(person, key); return removal?.can_inspect === true ? [this.deletedEnvironment(person, key)] : []; });
      return { status: 200, body: this.page(rows, query) };
    }
    if ((found = /^\/environments\/([^/]+)\/(restore|purge)$/.exec(path)) && ((found[2] === 'restore' && method === 'POST') || (found[2] === 'purge' && method === 'DELETE'))) {
      const key = found[1] ?? ''; const facts = this.environmentRemoval(person, key); if (facts === null || facts.phase === 'purged') throw new Refusal(410, 'gone', 'the kept files are gone');
      const purge = found[2] === 'purge'; const permitted = purge ? facts.can_purge : facts.can_restore;
      if (facts.phase === 'purging' || facts.phase === 'restoring') throw new Refusal(409, 'resource.in_use', 'a retention job already holds these volumes', { job: facts.job });
      if (!permitted) throw new Refusal(403, 'authz.denied', purge ? 'only an administrator of the home may remove files for good' : 'environment restoration authority required', { required: purge ? 'home_admin' : 'environment.manage', missing: [] });
      const expected = purge ? Number(query.get('expected_revision')) : body.expected_revision;
      if (expected !== facts.revision) throw new Refusal(409, 'conflict.stale_revision', 'the removal record changed', { current: facts });
      if (purge && query.get('confirm') !== this.environment(person, key).name) throw new Refusal(400, 'request.invalid', 'name the environment');
      return { status: 202, body: this.beginJob(person, key, purge ? 'environment_purge' : 'environment_restore') };
    }
    if ((found = /^\/jobs\/([^/]+)$/.exec(path)) && method === 'GET' && this.jobs.has(found[1] ?? '')) {
      const job = this.jobs.get(found[1] ?? '')!; const env = this.environment(person, job.environment_id ?? '');
      if (!env.access.permissions.includes('environment.manage') && !this.world.places.access(person, env.home.id).administers) throw new Refusal(403, 'authz.denied', 'environment progress authority required');
      this.advance(job); return { status: 200, body: job };
    }
    if (/^\/environments\/([^/]+)\/settings$/.test(path) && method === 'GET') {
      const old = this.world.resources.handle(person, method, path, query, body)?.body as unknown[];
      return { status: 200, body: [...old, { key: 'env.removed_keep_days', value: this.keepDays, source: { scope: 'default', node: null }, own: null, locked_by: null, writable: true }] };
    }
    if (path === '/environments' && method === 'GET') { const answer = this.world.resources.handle(person, method, path, query, body); return answer === null ? null : { ...answer, body: (answer.body as RetainedEnvironment[]).filter((env) => env.status !== 'removed') }; }
    return null;
  }
  private seedRetained(): void {
    const tree = this.world.trees.get(ACME.org); const ada = this.world.people.get('ada');
    if (tree === undefined || ada === undefined) return;
    const make = (key: string, kind: TreeNode['kind'], name: string, parent: string, source: string) => {
      const template = tree.nodes.get(source); if (template === undefined) throw new Error('the shared node seed is missing');
      const row = { ...structuredClone(template), id: key, kind, name, parent_id: parent, archived: true, deleted: false };
      tree.nodes.set(key, row); return row;
    };
    const project = make(TRACE.project, 'project', 'Archived experiment', ACME.web, ACME.web);
    const topic = make(TRACE.topic, 'topic', 'Removed topic', ACME.web, ACME.fixLogin);
    const child = make(TRACE.child, 'topic', 'Kept topic', TRACE.project, ACME.fixLogin);
    const thread = make(TRACE.thread, 'thread', 'Deleted investigation', ACME.fixLogin, ACME.mainThread);
    const played = sceneOf('history', TRACE.thread);
    for (const step of played.steps) this.world.threads.emit(TRACE.thread, step);
    for (const [item, text] of Object.entries(played.outputs)) this.world.threads.wholeOutput.set(item, text);
    const deletedBy = this.world.places.userOfName('dora');
    const inspection: Inspection = { deleted_at: EVENT_DAY, deleted_by: deletedBy, purge_at: '2026-11-11T12:00:00.000Z', readonly: true };
    for (const row of [project, topic, child, thread]) {
      const node = this.world.places.node(ada, row.id);
      const held: Retained = { kind: row.kind, name: row.name, home: row.parent_id, state: 'deleted', inspection, node };
      if (row.kind === 'thread') held.thread = this.world.threads.handle(ada, 'GET', `/threads/${row.id}/sync`, new URLSearchParams({ limit: '500' }), {})?.body as ThreadSync;
      // The thread's frames replaced its row in the tree: it is the row the tree holds now that is deleted.
      this.retained.set(row.id, held); (tree.nodes.get(row.id) ?? row).deleted = true;
    }
    const made = this.world.documents.handle(ada, 'POST', `/nodes/${ACME.fixLogin}/documents`, new URLSearchParams(), { title: 'Investigation notes', blocks: [{ type: 'heading', props: { level: 1 }, content: [{ type: 'text', text: 'Investigation' }] }, { type: 'paragraph', props: {}, content: [{ type: 'text', text: 'The model and the workspace were used separately.' }] }] });
    const documentId = (made?.body as { id: string }).id; const doc = this.world.documents.docs.get(documentId);
    if (doc !== undefined) {
      this.world.documents.docs.delete(documentId); doc.id = TRACE.document; this.world.documents.docs.set(doc.id, doc);
      for (const block of doc.blocks.values()) block.document_id = doc.id;
      const snapshot = this.world.documents.sync(ada, doc, null);
      this.retained.set(doc.id, { kind: 'document', name: doc.title, home: doc.nodeId, state: 'deleted', inspection, document: snapshot }); doc.deleted = true;
    }
    this.retained.set(TRACE.purged, { kind: 'thread', name: 'Purged draft', home: ACME.fixLogin, state: 'purged', inspection });
    for (const [key, held] of this.retained) {
      this.records.unshift({ ancestry: [...this.world.places.pathTo(held.home).map((place) => place.id), key], entry: { as_admin: false, request_id: null, event_deletion_id: null, id: id(28, this.records.length + 1), at: EVENT_DAY, actor: { kind: 'user', user: deletedBy }, action: held.kind === 'document' ? 'document.deleted' : 'node.deleted', kind: 'deleted', object: { ...lifecycle, id: key, kind: held.kind, name: held.name, state: held.state, deleted_at: inspection.deleted_at, deleted_by: inspection.deleted_by, purge_at: inspection.purge_at, purged_at: held.state === 'purged' ? '2026-10-13T08:00:00.000Z' : null, can_open: false, can_inspect: false, can_restore: false }, node: { id: held.home, kind: this.world.places.rowOf(held.home)?.kind ?? 'topic', name: this.world.places.rowOf(held.home)?.name ?? null, redacted: false }, history: true, override: false, summary: `${deletedBy.display_name} deleted ${held.name}`, details: { version: 1, name: held.name }, ip: null } });
    }
  }
  private mayInspect(person: Person, key: string): boolean {
    const held = this.retained.get(key); if (held === undefined || held.state === 'purged') return false;
    const denied = this.world.places.denied.get(person.me.username);
    if (held.kind === 'document') {
      const doc = this.world.documents.docs.get(key);
      return doc !== undefined && this.world.documents.full(person, doc).access.permissions.includes('doc.manage') && !denied?.has('doc.manage') && !denied?.has(`doc.manage@${key}`);
    }
    return this.world.places.access(person, held.home).permissions.includes(`${this.world.places.rowOf(held.home)?.kind ?? 'org'}.manage`);
  }
  private inspect(person: Person, key: string): Retained {
    const held = this.retained.get(key);
    if (held === undefined) throw new Refusal(404, 'not_found', 'no retained thing');
    // Authority is checked before the purged fact can reveal a concealed name.
    const allowed = held.state === 'purged' ? this.world.places.access(person, held.home).permissions.includes(`${this.world.places.rowOf(held.home)?.kind ?? 'org'}.manage`) : this.mayInspect(person, key);
    if (!allowed) throw new Refusal(403, 'authz.denied', 'restoration authority is required', { required: held.kind === 'document' ? 'doc.manage' : `${this.world.places.rowOf(held.home)?.kind ?? 'org'}.manage`, missing: [{ object: { kind: held.kind, id: key, name: null, owner: this.world.places.userOfName('ada') }, permissions: [held.kind === 'document' ? 'doc.manage' : `${this.world.places.rowOf(held.home)?.kind ?? 'org'}.manage`], options: null, ask: [this.world.places.userOfName('ada')], ask_more: 0, override: false, closed: null }] });
    if (held.state === 'purged') throw new Refusal(410, 'gone', 'the retained content is gone');
    const once = `inspect:${person.me.id}:${key}:${new Date().toISOString().slice(0, 10)}`;
    if (!this.inspections.has(once)) { this.inspections.add(once); this.records.unshift({ ancestry: this.world.places.pathTo(held.home).map((row) => row.id), entry: { as_admin: false, request_id: null, event_deletion_id: null, id: id(28, this.records.length + 1), at: new Date().toISOString(), actor: { kind: 'user', user: userRef(person.me) }, action: 'deleted.inspected', kind: 'access', object: { ...lifecycle, id: key, kind: held.kind, name: held.name }, node: null, history: false, override: false, summary: `Inspected ${held.name}`, details: null, ip: null } }); }
    return held;
  }
  private need(person: Person, scope: string | null): void {
    if (scope === null) { if (!person.me.is_system_admin) throw new Refusal(403, 'authz.denied', 'system administrator required', { required: 'system_admin', missing: [] }); }
    else this.world.places.need(person, scope, 'audit.view');
  }
  private filtered(person: Person, scope: string | null, query: URLSearchParams): TraceEntry[] {
    this.need(person, scope);
    const kind = query.get('kind'); if (kind !== null && !(kind in ACTIONS)) throw new Refusal(400, 'request.invalid', 'unknown audit kind');
    const actor = query.get('actor'); const includeAgents = query.get('include_agents') === 'true'; const action = query.get('action'); const from = query.get('from'); const to = query.get('to');
    const rows = this.records.filter(({ entry, ancestry }) => (scope === null || ancestry.includes(scope)) && (actor === null || ((entry.actor.kind === 'user' && entry.actor.user.id === actor) || (includeAgents && entry.actor.kind === 'agent' && entry.actor.for_user?.id === actor))) && (action === null || entry.action === action) && (kind === null || entry.kind === kind) && (from === null || entry.at >= from) && (to === null || entry.at <= to)).sort((a, b) => b.entry.at.localeCompare(a.entry.at) || b.entry.id.localeCompare(a.entry.id));
    const environments = rows.map((row) => {
      const entry = row.entry; const key = entry.object.id;
      if (entry.object.kind !== 'environment' || key === null) return row;
      const removal = this.environmentRemoval(person, key); const env = this.environment(person, key);
      return { ...row, entry: { ...entry, object: { ...lifecycle, ...entry.object, deletion_id: this.deletionIds.get(key) ?? null, state: removal === null ? 'live' as const : removal.phase === 'purged' ? 'purged' as const : 'deleted' as const, deleted_at: removal?.deleted_at ?? null, deleted_by: removal?.deleted_by ?? null, purge_at: removal?.purge_at ?? null, purged_at: removal?.purged_at ?? null, can_open: removal === null && env.access.permissions.includes('environment.view'), can_inspect: removal?.can_inspect ?? false, can_restore: removal?.can_restore ?? false } } };
    });
    return environments.map(({ entry }) => { if (entry.object.kind === 'environment') return entry;
      if (entry.action === 'admin.opened' && entry.object.kind === 'document' && entry.object.id !== null) {
        let current: ReturnType<World['documents']['openingAccess']> = null;
        try { current = this.world.documents.openingAccess(person, entry.object.id); }
        catch (error) { if (!(error instanceof Refusal) || error.answer.status !== 404) throw error; }
        return { ...entry, object: { ...entry.object, name: current?.name ?? null, can_open: current !== null, can_inspect: false, can_restore: false } };
      }
      const key = entry.object.id; const held = key === null ? undefined : this.retained.get(key); const live = held === undefined && entry.object.state === 'live' && key !== null; const can = live && this.world.places.access(person, key).permissions.includes(`${entry.object.kind}.view`); const inspect = key !== null && this.mayInspect(person, key); return { ...entry, object: { ...entry.object, deletion_id: key === null ? null : this.deletionIds.get(key) ?? null, ...(held === undefined ? {} : { state: held.state, deleted_at: held.inspection.deleted_at, deleted_by: held.inspection.deleted_by, purge_at: held.inspection.purge_at, purged_at: held.state === 'purged' ? '2026-10-13T08:00:00.000Z' : null }), can_open: can, can_inspect: inspect, can_restore: inspect } }; });
  }
  private page<T>(rows: T[], query: URLSearchParams) { const offset = Number(query.get('cursor') ?? 0); const limit = Number(query.get('limit') ?? PAGE_DEFAULT); return { items: rows.slice(offset, offset + limit), next_cursor: offset + limit < rows.length ? String(offset + limit) : null }; }
  async handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>, actKey?: string): Promise<Answer | null> {
    this.requests.push({ method, path, query: Object.fromEntries(query), body, idempotency_key: actKey ?? null });
    const replayKey = actKey === undefined ? null : `${person.me.id}:${method}:${path}:${actKey}`;
    const replay = replayKey === null ? undefined : this.completedActs.get(replayKey);
    if (replay !== undefined) return replay;
    if (query.get('inspect_deleted') === 'true' && method !== 'GET') throw new Refusal(400, 'request.invalid', 'inspection is read-only');
    const retention = this.retentionHandle(person, method, path, query, body); if (retention !== null) { if (replayKey !== null && method !== 'GET') this.completedActs.set(replayKey, retention); return retention; }
    let found: RegExpExecArray | null;
    if (query.get('inspect_deleted') === 'true') {
      if (method !== 'GET') throw new Refusal(400, 'request.invalid', 'inspection is read-only');
      if ((found = /^\/nodes\/([^/]+)(\/children)?$/.exec(path))) {
        const held = this.inspect(person, found[1] ?? '');
        if (found[2]) { const children = [...this.retained.entries()].filter(([, child]) => child.home === found?.[1] && this.mayInspect(person, child.node?.id ?? '')).map(([, child]) => ({ ...child.node, inspection: child.inspection })); return { status: 200, body: this.page(children, query) }; }
        return { status: 200, body: { ...held.node, inspection: held.inspection } };
      }
      if ((found = /^\/threads\/([^/]+)\/(sync|items)(?:\/([^/]+)\/output)?$/.exec(path))) {
        const held = this.inspect(person, found[1] ?? ''); const sync = held.thread;
        if (sync === undefined) throw new Refusal(404, 'not_found', 'no retained conversation');
        if (found[3]) return { status: 200, text: this.world.threads.wholeOutput.get(found[3]) ?? '' };
        if (found[2] === 'sync') return { status: 200, body: { ...sync, thread: { ...sync.thread, inspection: held.inspection } } };
        const before = Number(query.get('before') ?? Infinity); const limit = Number(query.get('limit') ?? 200); const items = sync.items.filter((item) => item.seq_created < before); return { status: 200, body: { items: items.slice(-limit), has_older: items.length > limit, has_newer: true, inspection: held.inspection } };
      }
      if ((found = /^\/documents\/([^/]+)(\/sync)?$/.exec(path))) { const held = this.inspect(person, found[1] ?? ''); if (held.document === undefined) throw new Refusal(404, 'not_found', 'no retained page'); return { status: 200, body: found[2] ? { ...held.document, document: { ...held.document.document, inspection: held.inspection } } : { ...held.document.document, inspection: held.inspection } }; }
      throw new Refusal(400, 'request.invalid', 'this request cannot inspect deleted content');
    }
    if ((found = /^\/nodes\/([^/]+)\/documents$/.exec(path)) && method === 'GET' && query.get('deleted') === 'only') {
      const page = this.world.documents.list(person, found[1] ?? '', query);
      return { status: 200, body: { ...page, items: page.items.map((doc) => { const held = this.retained.get(doc.id); const may = this.mayInspect(person, doc.id); return { ...doc, ...(held === undefined ? {} : { deleted_at: held.inspection.deleted_at, deleted_by: held.inspection.deleted_by, purge_at: held.inspection.purge_at }), can_inspect: may, can_restore: may }; }) } };
    }
    if ((found = /^\/nodes\/([^/]+)\/deleted$/.exec(path)) && method === 'GET') {
      const node = found[1] ?? ''; this.world.places.need(person, node, `${this.world.places.rowOf(node)?.kind ?? 'org'}.manage`);
      const ordinary = this.world.places.deletedUnder(person, node, query);
      const known = ordinary.items.filter((entry) => !this.retained.has(entry.node.id)).map((entry) => ({ ...entry, can_inspect: this.world.places.access(person, node).permissions.includes(`${this.world.places.rowOf(node)?.kind ?? 'org'}.manage`), can_restore: true }));
      const rows = [...this.retained.entries()].filter(([, held]) => held.home === node && held.kind !== 'document' && held.state === 'deleted').map(([key, held]) => ({ node: { id: key, kind: held.kind, name: held.name, redacted: false }, kind: held.kind, deleted_at: held.inspection.deleted_at, deleted_by: held.inspection.deleted_by, purge_at: held.inspection.purge_at, children: [...this.retained.values()].filter((child) => child.home === key).length, can_inspect: this.mayInspect(person, key), can_restore: this.mayInspect(person, key) }));
      return { status: 200, body: this.page([...known, ...rows], query) };
    }
    if ((found = /^\/(nodes|documents)\/([^/]+)\/restore$/.exec(path)) && method === 'POST' && this.retained.has(found[2] ?? '')) {
      const key = found[2] ?? ''; const held = this.retained.get(key)!;
      if (!this.mayInspect(person, key)) this.inspect(person, key);
      if (body.expected_deletion_id !== undefined && body.expected_deletion_id !== this.deletionIds.get(key)) throw new Refusal(409, 'conflict.stale_revision', 'the deletion identity changed', { current: { deletion_id: this.deletionIds.get(key), deleted_at: held.inspection.deleted_at } });
      const parent = this.retained.get(held.home);
      if (parent?.state === 'deleted') throw new Refusal(409, 'node.restore_parent_first', `Bring back ${parent.name} first`, { parent: { id: held.home, name: parent.name, kind: parent.kind, redacted: false } });
      const groupId = this.deletionIds.get(key);
      const group = [...this.retained].filter(([childKey, child]) => child.state === 'deleted' && (childKey === key || (held.kind !== 'document' && this.deletionIds.get(childKey) === groupId && (child.node?.path ?? child.document?.document.path ?? []).some((ref) => ref.id === key))));
      for (const [childKey] of group) if (!this.mayInspect(person, childKey)) throw new Refusal(403, 'authz.denied', 'current restoration authority is missing from a group member');
      for (const [childKey, child] of group) {
        this.retained.delete(childKey); this.deletionIds.delete(childKey);
        for (const { entry } of this.records) if (entry.object.id === childKey) entry.object = { ...entry.object, state: 'live', deletion_id: null, deleted_at: null, deleted_by: null, purge_at: null, purged_at: null };
        if (child.kind === 'document') { const doc = this.world.documents.docs.get(childKey); if (doc !== undefined) doc.deleted = false; }
        else { const row = this.world.places.rowOf(childKey); if (row !== undefined) { row.deleted = false; row.archived = true; } }
        this.records.unshift({ ancestry: this.world.places.pathTo(child.home).map((ref) => ref.id), entry: { as_admin: false, id: id(32, this.records.length + 1), at: new Date().toISOString(), actor: { kind: 'user', user: userRef(person.me) }, action: child.kind === 'document' ? 'document.restored' : 'node.restored', kind: 'changed', object: { ...lifecycle, id: childKey, kind: child.kind, name: child.name, state: 'live', deletion_id: null, can_open: true, can_inspect: false, can_restore: false }, node: null, summary: `Brought back ${child.name}`, details: { version: 1, name: child.name }, history: true, override: false, ip: null, request_id: actKey ?? null, event_deletion_id: null } });
      }
      const doc = this.world.documents.docs.get(key);
      const answer: Answer = { status: 200, body: held.kind === 'document' ? (doc === undefined ? held.document?.document : this.world.documents.full(person, doc)) : this.world.places.node(person, key) };
      if (replayKey !== null) this.completedActs.set(replayKey, answer);
      return answer;
    }
    const place = /^\/nodes\/([^/]+)\/audit(\/export)?$/.exec(path); const system = /^\/admin\/audit(\/export)?$/.exec(path);
    if (method === 'GET' && (place !== null || system !== null)) {
      const scope = place?.[1] ?? null; this.need(person, scope);
      if (this.delayMs > 0) await new Promise((resolve) => setTimeout(resolve, this.delayMs));
      this.need(person, scope);
      if (this.failing) throw new Refusal(503, 'internal', 'trail temporarily unavailable');
      const rows = this.filtered(person, scope, query);
      if (place?.[2] || system?.[1]) {
        const from = Date.parse(query.get('from') ?? ''); const to = Date.parse(query.get('to') ?? '');
        if (!Number.isFinite(from) || !Number.isFinite(to) || to < from || to - from > EXPORT_DAYS * DAY_MS) throw new Refusal(400, 'request.invalid', 'export span must be at most 366 days');
        const format = query.get('format'); if (format !== 'csv' && format !== 'jsonl') throw new Refusal(400, 'request.invalid', 'unknown file format');
        const escape = (value: string) => `"${value.replaceAll('"', '""')}"`;
        const text = format === 'jsonl' ? rows.map((row) => JSON.stringify(row)).join('\n') + '\n' : 'at,actor,action,kind,object\r\n' + rows.map((row) => [row.at, row.actor.kind === 'user' ? row.actor.user.display_name : row.actor.kind, row.action, row.kind ?? '', row.object.name ?? ''].map(escape).join(',')).join('\r\n');
        this.records.unshift({ ancestry: scope === null ? [] : [scope], entry: { as_admin: false, request_id: null, event_deletion_id: null, id: id(26, this.records.length + 1), at: new Date().toISOString(), actor: { kind: 'user', user: userRef(person.me) }, action: 'audit.exported', kind: declaredKind('audit.exported'), object: { ...lifecycle, kind: scope === null ? 'system' : 'node', id: scope }, node: null, history: false, override: false, ip: null, summary: 'Trail downloaded', details: Object.fromEntries(query) } });
        return { status: 200, bytes: new TextEncoder().encode(text), headers: { 'Content-Type': format === 'csv' ? 'text/csv' : 'application/x-ndjson', 'Content-Disposition': `attachment; filename="audit.${format}"`, ...(this.incomplete ? { 'X-Orch-Incomplete': 'true' } : {}) } };
      }
      return { status: 200, body: this.page(rows, query) satisfies TracePage };
    }
    return null;
  }
  control(path: string, body: Record<string, unknown>): Answer | null {
    if (path === '/state') return { status: 200, body: { requests: this.requests, records: this.records, frames: this.frames, deletion_ids: Object.fromEntries(this.deletionIds) } };
    if (path === '/configure') { if (typeof body.delayMs === 'number') this.delayMs = body.delayMs; if (typeof body.failing === 'boolean') this.failing = body.failing; if (typeof body.incomplete === 'boolean') this.incomplete = body.incomplete; if (body.empty === true) this.records = []; return { status: 204 }; }
    if (path === '/deletion') {
      const key = typeof body.id === 'string' ? body.id : '';
      if (body.redelete === true && !this.retained.has(key)) {
        const previous = this.retainedArchive.get(key); if (previous !== undefined) { this.retained.set(key, structuredClone(previous)); const node = this.world.places.rowOf(key); if (node !== undefined) node.deleted = true; const doc = this.world.documents.docs.get(key); if (doc !== undefined) doc.deleted = true; }
      }
      const held = this.retained.get(key);
      if (held !== undefined) {
        if (body.purged === true) held.state = 'purged';
        if (body.redelete === true) {
          const deletionId = id(31, ++this.deletionSequence); this.deletionIds.set(key, deletionId);
          this.records.unshift({ ancestry: [...this.world.places.pathTo(held.home).map((ref) => ref.id), key], entry: { as_admin: false, id: id(35, this.deletionSequence), at: held.inspection.deleted_at, actor: { kind: 'user', user: this.world.places.userOfName('dora') }, action: held.kind === 'document' ? 'document.deleted' : 'node.deleted', kind: 'deleted', request_id: null, event_deletion_id: deletionId, object: { ...lifecycle, id: key, kind: held.kind, name: held.name, state: 'deleted', deletion_id: deletionId, can_restore: true, can_inspect: true, can_open: false }, node: null, summary: `Deleted ${held.name} again`, details: { version: 1, name: held.name, deletion_id: deletionId }, history: true, override: false, ip: null } });
        }
        if (body.unknown === true) for (const { entry } of this.records) if (entry.object.id === key) entry.event_deletion_id = null;
        if (body.live === true) { this.retained.delete(key); this.deletionIds.delete(key); const row = this.world.places.rowOf(key); if (row !== undefined) row.deleted = false; for (const { entry } of this.records) if (entry.object.id === key) entry.object.state = 'live'; }
      }
      return { status: 204 };
    }
    if (path === '/investigation') {
      const dora = this.world.places.userOfName('dora'); const grace = this.world.places.userOfName('grace');
      const retained = this.records.filter(({ entry }) => entry.object.state === 'deleted' || entry.object.state === 'purged');
      this.records = retained;
      const node = { id: TRACE.thread, name: 'Thread name recorded then', kind: 'thread' as const, redacted: false };
      const add = (index: number, actor: TraceEntry['actor'], action: string, requestId: string | null, details: unknown) => {
        const objectId = action === 'workspace.changed_by_agent' ? TRACE.thread : ACME.mainThread;
        const summary = action === 'workspace.changed_by_agent' ? (typeof details === 'object' && details !== null && 'observation' in details && details.observation === 'unavailable' ? 'File observation was unavailable' : 'The agent changed the workspace') : 'Shared the investigation with Earlier group name';
        this.records.push({ ancestry: this.world.places.pathTo(objectId).map((ref) => ref.id), entry: { as_admin: false, id: id(33, index), at: '2026-10-12T13:00:00.000Z', actor, action, kind: declaredKind(action), request_id: requestId, event_deletion_id: null, object: { ...lifecycle, id: objectId, kind: 'thread', name: node.name, state: 'live', can_open: true, can_inspect: false, can_restore: false }, node: { ...node, id: objectId }, summary, details, history: true, override: false, ip: null } });
      };
      // This one real-shaped request spans the normal page boundary; its exact entries remain independent.
      for (let index = 1; index <= 45; index += 1) add(index, { kind: 'user', user: dora }, 'permission.added', 'one-share-request', { version: 1, holder: { id: id(34, 1), name: 'Earlier group name', kind: 'group', place: 'Web' }, permission: index % 2 === 0 ? 'thread.view' : 'files.view', profile: index % 2 === 0 ? 'managed' : 'connected', options: { can_download: index % 2 === 0 } });
      add(46, { kind: 'user', user: grace }, 'permission.added', 'different-share-request', { holder: { name: 'Earlier group name', kind: 'group' }, profile: 'managed' });
      add(47, { kind: 'agent', thread: node, for_user: dora }, 'workspace.changed_by_agent', 'turn-for-dora', { paths: ['result.txt'], observation: 'complete' });
      add(48, { kind: 'agent', thread: node, for_user: grace }, 'workspace.changed_by_agent', 'turn-for-grace', { paths: ['other.txt'], observation: 'complete' });
      add(49, { kind: 'agent', thread: node, for_user: null }, 'workspace.changed_by_agent', null, { observation: 'unavailable' });
      add(50, { kind: 'user', user: dora }, 'permission.added', null, { holder: { name: 'Earlier group name', kind: 'group' } });
      return { status: 204 };
    }
    if (path === '/retention') {
      if (typeof body.keep_days === 'number') this.keepDays = body.keep_days;
      if (typeof body.hold_jobs === 'boolean') this.holdJobs = body.hold_jobs;
      if (typeof body.failure === 'string' || body.failure === null) this.retentionFailure = body.failure;
      if (typeof body.username === 'string') for (const act of ['inspect', 'restore', 'purge']) { if (body[`can_${act}`] === false) this.barred.add(`${body.username}:${act}`); else if (body[`can_${act}`] === true) this.barred.delete(`${body.username}:${act}`); }
      if (typeof body.environment === 'string') { const facts = this.removals.get(body.environment); if (facts !== undefined) { if (body.stale === true) facts.revision += 1; if (body.phase === 'purging') this.beginJob(this.world.people.get('ada')!, body.environment, 'environment_purge'); } }
      if (typeof body.host_manager === 'string') { const host = this.world.resources.hosts[0]; if (host !== undefined && !host.manages.includes(body.host_manager)) host.manages.push(body.host_manager); }
      return { status: 204 };
    }
    return null;
  }
  frame(_subscriber: unknown, _person: Person | undefined, frame: Record<string, unknown>): boolean { this.frames.push(frame); return false; }
}
