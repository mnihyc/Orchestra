/** First-steps facts read from the one fake world's providers, work, members and ordinary documents. */

import { firstStepsChoice, landingOrg, record, savedOrg } from '../../src/features/onboarding/state.ts';
import type { DocumentRef, Org, PermissionPage, Provider, Role, ThreadSync, TreeNode, FirstSteps, InvitationRoleSummary } from '../../src/generated/api-types.ts';
import { agentMessage, turn } from './thread-rows.ts';
import { Refusal, type Answer, type Part, type Person, type World } from './world.ts';

const ROLES: Role[] = ['viewer', 'interactor', 'developer', 'admin'];
const ref = (row: TreeNode | Org) => ({ id: row.id, kind: row.kind, name: row.name, redacted: false });
type Step = FirstSteps['steps'][number];

/** No independent setup ticks: controls change the same rows the ordinary screens read. */
export class FirstStepsWorld implements Part {
  reads = 0;
  delayMs = 0;
  failed = false;
  unknown = false;
  scriptedThreads = new Set<string>();
  guideIds = new Map<string, string>();

  private world: World;
  constructor(world: World) { this.world = world; }

  private state(person: Person): Record<string, unknown> {
    return record(this.world.settings.handle(person, 'GET', '/me/state', new URLSearchParams(), {})?.body);
  }

  private org(person: Person, requested: string | null): Org | null {
    const orgs = this.world.orgsOf(person);
    const state = this.state(person);
    const selected = requested ?? firstStepsChoice(state).org_id ?? savedOrg(state);
    let found = selected === null ? landingOrg(orgs, person.me.personal_org_id, null) : orgs.find((entry) => entry.id === selected);
    if (found === undefined && selected !== null && this.world.places.rowOf(selected)?.kind === 'org') {
      this.world.places.need(person, selected, 'org.view');
      found = this.world.places.node(person, selected) as Org;
    }
    if (found === undefined) {
      if (requested !== null) throw new Refusal(404, 'not_found', 'no such org');
      return landingOrg(orgs, person.me.personal_org_id, null) ?? null;
    }
    this.world.places.need(person, found.id, 'org.view');
    return found;
  }

  private action(person: Person, org: Org, url: string, permission: string | null): Step['action'] {
    const allowed = permission === null || this.world.places.access(person, org.id).permissions.includes(permission);
    return { url, allowed, reason: allowed ? null : `${permission} is needed. Ask ${org.owner.display_name}.` };
  }

  private roles(person: Person, org: Org): InvitationRoleSummary[] {
    const answer = this.world.access.handle(person, 'GET', `/nodes/${org.id}/defaults`, new URLSearchParams(), {});
    const page = answer?.body as PermissionPage;
    const permits = (role: Role, permission: string): boolean => {
      const row = page.rows.find((entry) => entry.permission === permission && entry.profile !== 'connected');
      const holders = [
        ...(row?.inherited?.holders ?? []), ...(row?.entries.map((entry) => entry.holder) ?? []),
      ];
      return holders.some((holder) => holder.kind === 'everyone' || (holder.kind === 'role' && ROLES.indexOf(role) >= ROLES.indexOf(holder.role)));
    };
    return ROLES.map((role) => ({ role, start_topics: permits(role, 'project.create_topics') || permits(role, 'project.interact'), start_threads: permits(role, 'topic.start_threads') || permits(role, 'topic.interact'), send_inputs: permits(role, 'thread.interact'), answer_requests: permits(role, 'thread.manage'), model_access_required: true }));
  }

  private guide(person: Person, org: Org): DocumentRef | null {
    const id = this.guideIds.get(org.id);
    if (id === undefined) return null;
    const held = this.world.documents.docs.get(id);
    // The old document double grants administrators implicit inspection; setup never borrows it.
    if (held?.page === 'restricted' && held.owner !== person.me.username) return null;
    try {
      const answer = this.world.documents.handle(person, 'GET', `/documents/${id}`, new URLSearchParams(), {});
      const document = answer?.body as DocumentRef | undefined;
      if (document?.deleted !== false) return null;
      // A selected guide is independent of where it falls in a paged list; return metadata alone.
      return {
        archived: document.archived, block_count: document.block_count, bytes: document.bytes,
        created_by: document.created_by, deleted: document.deleted, discussion_level: document.discussion_level,
        files_bytes: document.files_bytes,
        id: document.id, node: document.node, open_discussions: document.open_discussions,
        description: document.description, description_source: document.description_source,
        description_line: document.description_line,
        owner: document.owner, owners: document.owners, page: document.page, seen_seq: document.seen_seq,
        seq: document.seq, title: document.title, updated_at: document.updated_at,
        updated_by: document.updated_by, version: document.version,
      };
    } catch (error) {
      if (error instanceof Refusal && [403, 404, 410].includes(error.answer.status)) return null;
      throw error;
    }
  }

  projection(person: Person, requested: string | null): FirstSteps {
    const org = this.org(person, requested);
    if (org === null) return { org: null, work_mode: null, steps: [], invitation_roles: [], start_here: null, as_of: new Date().toISOString() };
    const choice = firstStepsChoice(this.state(person));
    const mode = choice.org_id === org.id ? choice.work_mode : null;
    const visible = [...(this.world.trees.get(org.org_id)?.nodes.values() ?? [])].filter((node) =>
      !node.deleted && this.world.places.pathTo(node.id).some((ancestor) => ancestor.id === org.id) && this.world.places.access(person, node.id).permissions.includes(`${node.kind}.view`));
    const ids = new Set(visible.map((node) => node.id));
    ids.add(org.id);
    const settings = this.world.settings.handle(person, 'GET', `/nodes/${org.id}/settings`, new URLSearchParams(), {})?.body as { key: string; value: unknown }[];
    const defaultProvider = settings.find((entry) => entry.key === 'agent.default_provider_id')?.value;
    const readableProviders: Provider[] = [];
    for (const candidate of this.world.resources.providers) {
      if (!ids.has(candidate.home) && candidate.id !== defaultProvider) continue;
      try {
        const provider = this.world.resources.handle(person, 'GET', `/providers/${candidate.id}`, new URLSearchParams(), {})?.body as Provider;
        readableProviders.push(provider);
      } catch (error) {
        if (!(error instanceof Refusal) || ![403, 404].includes(error.answer.status)) throw error;
      }
    }
    const providers = readableProviders.filter((provider) => ids.has(provider.home.id) || (provider.id === defaultProvider && provider.access.permissions.includes('provider.use')));
    const configured = providers.find((provider) => provider.key_set || (provider.kind === 'subscription' && provider.facts.identity !== null) || (provider.kind === 'api' && provider.upstreams.some((upstream) => record(upstream.options).auth === 'none')));
    const models = providers.find((provider) => provider.models.some((model) => model.enabled));
    let answer: TreeNode | undefined;
    for (const node of visible) {
      if (node.kind !== 'thread') continue;
      const sync = this.world.threads.handle(person, 'GET', `/threads/${node.id}/sync`, new URLSearchParams(), {})?.body as ThreadSync;
      const completed = sync.turns.filter((entry) => entry.state === 'completed' && entry.error?.incomplete !== true);
      const answered = completed.some((entry) => {
        const messages = sync.items.filter((item) => item.kind === 'agent_message' && item.turn_id === entry.id).sort((one, other) => one.seq_created - other.seq_created);
        const finals = messages.filter((item) => item.kind === 'agent_message' && item.payload.phase === 'final');
        const candidate = finals.length > 0 ? finals.at(-1) : messages.every((item) => item.kind === 'agent_message' && item.payload.phase === null) ? messages.filter((item) => item.status === 'completed').at(-1) : undefined;
        return candidate?.kind === 'agent_message' && candidate.status === 'completed' && candidate.payload.text.trim() !== '';
      });
      if (answered) { answer = node; break; }
    }
    const step = (id: Step['id'], fact: Step['fact'], source: Step['source'], action: Step['action'], scripted = false): Step => ({
      id, fact: this.unknown && id !== 'team' ? 'unknown' : fact,
      state: this.unknown && id !== 'team' ? 'unknown' : ['choose_org', 'not_done', 'invitation_waiting'].includes(fact) ? 'open' : 'done',
      source: this.unknown ? null : source, action, scripted,
    });
    const fact: Step['fact'] = configured === undefined ? 'not_done' : configured.kind === 'subscription' ? 'signed_in' : configured.key_set ? 'key_saved' : 'endpoint_configured';
    const providerURL = configured === undefined ? `/resources/providers?org_id=${org.id}` : `/resources/providers/${configured.id}`;
    const steps = [
      step('team', mode === null ? 'choose_org' : mode === 'solo' ? 'solo_chosen' : 'team_chosen', ref(org), null),
      step('provider', fact, configured === undefined ? null : ids.has(configured.home.id) ? configured.home : null, this.action(person, org, providerURL, configured === undefined ? 'settings.manage' : null)),
      step('models', models === undefined ? 'not_done' : 'models_enabled', models === undefined ? null : ref(org), this.action(person, org, models === undefined ? `/resources/providers?org_id=${org.id}` : `/resources/providers/${models.id}?tab=models`, 'settings.manage')),
      step('work', answer === undefined ? 'not_done' : 'answered_once', answer === undefined ? null : ref(answer), this.action(person, org, answer === undefined ? `/org/${org.address}` : `/thread/${answer.id}`, answer === undefined ? 'org.interact' : null), answer !== undefined && this.scriptedThreads.has(answer.id)),
    ];
    // The current server has not supplied the shared outward-status helper; never fabricate a configured tick.
    if (answer !== undefined) steps.push({ ...step('away', 'unknown', null, { url: '/settings/notifications', allowed: true, reason: null }), state: 'unknown' });
    if (mode !== 'solo') {
      const members = this.world.places.memberList(person, org.id, new URLSearchParams()).items;
      const invitations = this.world.places.access(person, org.id).permissions.includes('members.manage') ? this.world.places.handle(person, 'GET', '/invitations', new URLSearchParams({ org_id: org.id }), {})?.body as { uses: number; revoked: boolean }[] | undefined : undefined;
      steps.push(step('teammate', members.length > 1 ? 'member_joined' : invitations?.some((entry) => entry.uses === 0 && !entry.revoked) ? 'invitation_waiting' : 'not_done', ref(org), this.action(person, org, `/org/${org.address}?tab=members`, 'members.manage')));
    }
    return { org: ref(org), work_mode: mode, steps, invitation_roles: this.roles(person, org), start_here: this.guide(person, org), as_of: new Date().toISOString() };
  }

  async handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Promise<Answer | null> {
    if (method === 'PUT' && path === '/me/first-steps') {
      if ((body.org_id !== null && typeof body.org_id !== 'string') || (body.work_mode !== null && body.work_mode !== 'solo' && body.work_mode !== 'team') || (body.org_id === null && body.work_mode !== null) || typeof body.dismissed !== 'boolean') throw new Refusal(400,'request.invalid','choose an org and work mode');
      if (typeof body.org_id === 'string') this.org(person,body.org_id);
      const state=this.state(person);
      const help={...record(state.help),first_steps:{org_id:body.org_id,work_mode:body.work_mode,dismissed:body.dismissed}};
      state.help=help;
      if (body.org_id !== null) state.sidebar={...record(state.sidebar),last_org_id:body.org_id};
      this.world.users.tell(person.me.username,{what:'state',key:'help',child:'first_steps',value:help.first_steps});
      if (body.org_id !== null) this.world.users.tell(person.me.username,{what:'state',key:'sidebar',child:'last_org_id',value:body.org_id});
      return {status:200,body:this.state(person)};
    }
    if (method === 'GET' && path === '/me/first-steps') {
      this.reads += 1;
      if (this.delayMs > 0) await new Promise((resolve) => setTimeout(resolve, this.delayMs));
      if (this.failed) throw new Refusal(503, 'internal', 'this org could not be checked');
      return { status: 200, body: this.projection(person, query.get('org_id')) };
    }
    const setter = /^\/nodes\/([^/]+)\/settings\/org.start_here_document_id$/.exec(path);
    if (method === 'PUT' && setter?.[1] !== undefined) {
      const org = this.org(person, setter[1]);
      if (org === null) throw new Refusal(404, 'not_found', 'no such org');
      this.world.places.need(person, org.id, 'org.manage');
      if (body.value === null) this.guideIds.delete(org.id);
      else {
        if (typeof body.value !== 'string') throw new Refusal(400, 'request.invalid', 'choose a document or clear this setting');
        const id = body.value;
        const doc = this.world.documents.handle(person, 'GET', `/documents/${id}`, new URLSearchParams(), {})?.body as DocumentRef;
        if (this.world.places.rootOf(doc.node.id) !== org.org_id) throw new Refusal(400, 'request.invalid', 'choose a readable document in this tree');
        this.guideIds.set(org.id, id);
      }
      return { status: 204 };
    }
    return null;
  }

  control(path: string, body: Record<string, unknown>): Answer | null {
    const person = this.world.people.get(typeof body.username === 'string' ? body.username : 'ada');
    if (person === undefined) throw new Refusal(404, 'not_found', 'no such person');
    if (path === '/set') {
      if (typeof body.delayMs === 'number') this.delayMs = body.delayMs;
      if (typeof body.failed === 'boolean') this.failed = body.failed;
      if (typeof body.unknown === 'boolean') this.unknown = body.unknown;
      if (typeof body.guide === 'string' && typeof body.org === 'string') this.guideIds.set(body.org, body.guide);
      if (body.guide === null && typeof body.org === 'string') this.guideIds.delete(body.org);
      return { status: 204 };
    }
    if (path === '/choice') {
      const state = this.state(person);
      this.world.settings.handle(person, 'PUT', '/me/state/help', new URLSearchParams(), { ...record(state.help), first_steps: { org_id: body.org, work_mode: body.mode ?? null, dismissed: body.dismissed === true } });
      return { status: 204 };
    }
    if (path === '/answer') {
      const thread = String(body.thread);
      const turnId = `first-steps-${thread}`;
      const item = agentMessage(`${turnId}-answer`, typeof body.text === 'string' ? body.text : 'A final answer.', { turn: turnId, phase: body.commentary === true ? 'commentary' : 'final', status: body.incomplete === true ? 'failed' : 'completed' });
      this.world.threads.emit(thread, { turns: [turn(turnId, { ordinal: 1, inputId: `${turnId}-input`, state: body.failed === true ? 'failed' : 'completed' })], items: [item] });
      if (body.scripted === true) this.scriptedThreads.add(thread);
      return { status: 204 };
    }
    if (path === '/state') return { status: 200, body: { reads: this.reads, state: this.state(person) } };
    if (path === '/appearance') {
      if (body.locale === 'en' || body.locale === 'zh-CN') person.me.locale = body.locale;
      if (body.theme === 'light' || body.theme === 'dark' || body.theme === 'system') person.me.theme = body.theme;
      return { status: 204 };
    }
    if (path === '/org') return this.world.places.handle(person, 'POST', '/orgs', new URLSearchParams(), { name: body.name, slug: body.slug });
    if (path === '/landing') {
      // The org last in view, and the place last in on any device ({ node_id, route }), each only when given.
      const state = this.state(person);
      state.sidebar = { ...record(state.sidebar), ...('org' in body ? { last_org_id: body.org } : {}), ...('place' in body ? { last_place: body.place } : {}) };
      return { status: 204 };
    }
    if (path === '/document') {
      const orgId = typeof body.org === 'string' ? body.org : person.me.personal_org_id;
      if (orgId === null) throw new Refusal(400, 'request.invalid', 'choose an organization for this document');
      const answer = this.world.documents.handle(person, 'POST', `/nodes/${orgId}/documents`, new URLSearchParams(), { title: body.title ?? 'Team welcome', markdown: 'Welcome to the team. Read this ordinary document beside your work.' });
      const document = answer?.body as DocumentRef;
      this.guideIds.set(orgId, document.id);
      const root = this.world.places.rootOf(orgId);
      if (root !== null) this.world.changeTree(root, [{ id: orgId }]);
      return { status: 200, body: { id: document.id } };
    }
    if (path === '/document-change') {
      const document = this.world.documents.docs.get(String(body.id));
      if (document === undefined) throw new Refusal(404, 'not_found', 'no such document');
      if (typeof body.title === 'string') document.title = body.title;
      if (body.hidden === true) { document.page = 'restricted'; document.owner = 'grace'; }
      if (body.deleted === true) document.deleted = true;
      // The joined tree follows the place's document count, including access loss.
      const root = this.world.places.rootOf(document.nodeId);
      if (root !== null) this.world.changeTree(root, [{ id: document.nodeId }]);
      return { status: 204 };
    }
    return null;
  }
}
