/** What the fake API says of the system's own screens: its people and root orgs, its settings, what it stores about itself, its hosts and runtimes, its capabilities, harness builds and agent profiles, its storage and authority, its audit trail, its plugins and its identity providers.
 *
 * It keeps to what the real API promises of these requests, as far as a screen can tell: everything
 * here is for a system admin and refused to anybody else with the same refusal; a person is
 * suspended at once and refused afterwards; the last system admin cannot be taken away; a change to a
 * system admin is acknowledged; and every act writes a line of the audit trail, which a test can
 * read back through the audit list and the export.
 */
import { randomUUID } from 'node:crypto';

import type {
  AdminPlugin,
  AdminUser,
  Agent,
  AuditEntry,
  CapabilityDef,
  HarnessBuild,
  HarnessChannelsStatus,
  HarnessKindInfo,
  Host,
  IdentityProvider,
  InstallationRecords,
  MachineRef,
  MetricSeries,
  Org,
  PkiInfo,
  ProgramSummary,
  ProgramDetail,
  ResolvedSetting,
  Runtime,
  StorageStats,
  SystemInfo,
  UserRef,
} from '../../src/generated/api-types.ts';
import { compareVersions } from '../../src/lib/versions.ts';
import { kindOf } from '../../src/features/trace/catalogue.ts';
import { SETTINGS_REGISTRY } from '../../src/generated/settings-registry.ts';
import { id as fixtureId, MADE_AT, me as makeMe, userRef } from './fixtures.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';
import { harnessName, partialDescriptor } from './harness-names.ts';

/** What an audit line says of a thing that is there and was never deleted, which the double's lines are about. */
const LIVE_OBJECT = { id: null, name: null, state: 'live' as const, deletion_id: null, deleted_at: null, deleted_by: null, purge_at: null, purged_at: null, can_open: false, can_inspect: false, can_restore: false };

const DAY_MS = 86_400_000;
const HOUR_MS = 3_600_000;
const MINUTE_MS = 60_000;
const PAGE_DEFAULT = 50;
const PAGE_MAX = 200;
/** Keep the full count beside at most fifty certificates due in the next thirty days. */
const PKI_MACHINES_LIMIT = 50;
const PKI_MACHINE_WARNING_DAYS = 30;
/** The most points a metric series has, and the steps it may have, as the real API holds them. */
const METRIC_MAX_POINTS = 720;
const METRIC_STEPS = [1, 5, 60, 1440];
/** How many days the value of each minute is kept. */
const METRICS_FINE_DAYS = 30;
const USERNAME = /^[a-z0-9][a-z0-9_-]{2,31}$/;

const text = (value: unknown): string => (typeof value === 'string' ? value : '');
const flag = (value: unknown): boolean => value === true;

/** Where the page of the application is served, for the links the double makes. */
const APP_ORIGIN = `http://127.0.0.1:${process.env.ORCH_E2E_PORT ?? '41872'}`;

function invalid(path: string, message: string): never {
  throw new Refusal(400, 'request.invalid', 'the request is not valid', { errors: [{ path, message }] });
}

// ---------------------------------------------------------------- settings of the system

interface Own {
  value: unknown;
  locked: boolean;
  revision: number;
}

/** What each setting of the system is when nobody set it, for the keys the screens use. */
const SETTING_DEFAULTS: Record<string, unknown> = {
  ...Object.fromEntries(SETTINGS_REGISTRY.filter((def) => def.key.startsWith('search.semantic.') || def.key === 'search.word_tuning').map((def) => [def.key, def.default])),
  'registration.mode': 'invite',
  'registration.email_verification': 'off',
  'auth.local_login': true,
  'auth.email_code': [],
  'orgs.personal_enabled': true,
  'orgs.personal_closed': true,
  'orgs.personal_flat': true,
  // The double's world lets anybody make a team org, as its scenarios were written; Orch ships `invited`.
  'orgs.created_by': 'anyone',
  'mail.shared': true,
  'outbound.allowed_private_hosts': [],
  'hosts.full_percent': 90,
  'switch.batch_threads': 200,
  'hosts.when_full': 'warn',
  'system.banner': null,
  'usage.catalogue_url': null,
  'usage.rates': {},
  'usage.catalogue': true,
  'usage.catalogue_scale': '1',
  'usage.catalogue_refresh_days': 0,
  'runtime.template_id': null,
  'release.check_days': 0,
  'agents.harness_versions': {},
  'agents.harness_channel_hours': 24,
  'backup.warn_days': 2,
};

// ---------------------------------------------------------------- the audit trail

const ACTORS = ['ada', 'grace', 'vera', 'dora', 'system'] as const;
const KINDS: readonly [action: string, verb: string, object: string][] = [
  ['user.registered', 'registered', 'Dora Developer'],
  ['user.suspended', 'suspended', 'Linus Pauling'],
  ['user.resumed', 'resumed', 'Linus Pauling'],
  ['org.limits_changed', 'changed the limits of', 'Acme'],
  ['setting.changed', 'changed the setting', 'hosts.full_percent'],
  ['provider.created', 'added the provider', 'Claude Max'],
  ['host.updated', 'changed the host', 'build-box'],
  ['environment.created', 'made the environment', 'Main'],
];
const SEEDED_ENTRIES = 130;

// ---------------------------------------------------------------- the world

export class AdminWorld implements Part {
  private channelsReadAt: string | null = null;
  private installation: InstallationRecords = { release: null, backup: null, backup_warning: true, backup_error: null };
  readonly world: World;
  own = new Map<string, Own>();
  entries: AuditEntry[] = [];
  seenAt = new Map<string, string>();
  deletedOrgs: Org[] = [];
  /** Root orgs this part made up, which are not in the tree of any person. */
  extraOrgs: Org[] = [];
  capabilities: CapabilityDef[] = [];
  builds: HarnessBuild[] = [];
  identityProviders: IdentityProvider[] = [];
  /** The state of the parts of the system, as a test sets it. */
  components: SystemInfo['components'] = [];
  /** Whether the system has stored any measurement; a test empties it. */
  stored = true;
  /** Who administers, by username, besides the people of the world: nobody. */
  private audited = 0;
  /** Who holds an unused invitation to make a team org, by username, with when it was given. */
  orgInvited = new Map<string, string>();

  constructor(world: World) {
    this.world = world;
    this.seed();
  }

  // ---------------------------------------------------------------- seeding

  private seed(): void {
    const now = Date.now();
    this.components = [
      { name: 'api', ok: true, detail: null, seen_at: null },
      { name: 'gateway', ok: true, detail: null, seen_at: new Date(now - 6_000).toISOString() },
      { name: 'worker', ok: true, detail: null, seen_at: new Date(now - 9_000).toISOString() },
      { name: 'database', ok: true, detail: null, seen_at: null },
    ];
    this.entries = Array.from({ length: SEEDED_ENTRIES }, (_, index): AuditEntry => {
      const [action, verb, object] = KINDS[index % KINDS.length] ?? KINDS[0] ?? ['setting.changed', 'changed', 'x'];
      const who = ACTORS[index % ACTORS.length] ?? 'ada';
      const person = who === 'system' ? undefined : this.world.people.get(who);
      const actor: AuditEntry['actor'] = person === undefined ? { kind: 'system' } : { kind: 'user', user: userRef(person.me) };
      const name = person === undefined ? 'The system' : person.me.display_name;
      return {
        id: fixtureId(28, index + 1),
        at: new Date(Date.parse(MADE_AT) - index * 47 * MINUTE_MS).toISOString(),
        action,
        actor,
        as_admin: false,
        override: index % 37 === 5,
        details: {},
        event_deletion_id: null,
        history: true,
        ip: person === undefined ? null : '203.0.113.7',
        kind: kindOf(action) ?? 'changed',
        node: null,
        object: { ...LIVE_OBJECT, kind: action.split('.')[0] ?? 'object' },
        request_id: null,
        summary: `${name} ${verb} ${object}`,
      };
    });
    this.audited = this.entries.length;
    this.seenAt.set('ada', new Date(now - 4 * MINUTE_MS).toISOString());
    this.seenAt.set('grace', new Date(now - DAY_MS).toISOString());
    this.seenAt.set('vera', new Date(now - 3 * DAY_MS).toISOString());

    const ada = this.world.people.get('ada');
    const owner = ada === undefined ? undefined : userRef(ada.me);
    if (owner !== undefined) {
      const orphan = fixtureId(2, 900);
      this.extraOrgs.push(
        this.root(orphan, 'orphans', 'Orphans Inc', owner, { owners: 0, member_count: 3, org_count: 1, project_count: 2, closed: false, flat: false }),
      );
      this.deletedOrgs.push(
        this.root(fixtureId(2, 901), 'old-labs', 'Old Labs', owner, { member_count: 4, project_count: 5, updated_at: new Date(now - 6 * DAY_MS).toISOString() }),
      );
    }

    this.capabilities = [
      cap('ptrace', 'Debugger access', 'medium', 'allow', 'host', 'Lets a debugger attach to the processes of the environment.'),
      cap('kvm', 'KVM virtualisation', 'high', 'ask', 'host', 'Gives the environment the host machine\'s virtualisation device.'),
      cap('gpu', 'GPU devices', 'medium', 'ask', 'host', 'Gives the environment the GPUs of the host.'),
      cap('net_admin', 'Network administration', 'high', 'ask', 'host', 'Lets the environment change its own network.'),
      cap('unconfined', 'No container sandbox', 'critical', 'ask', 'org', 'Runs the environment without the container\'s sandbox.'),
      cap('privileged', 'Privileged container', 'critical', 'deny', 'system', 'Runs the environment as a privileged container.'),
      cap('host_network', 'Host network', 'critical', 'deny', 'system', 'Puts the environment on the network of its host.'),
      cap('docker_socket', 'Host Docker socket', 'critical', 'deny', 'system', 'Gives the environment the Docker socket of its host.'),
    ];

    this.builds = [
      build('claude-code', '2.1.0', true, 'release'),
      build('claude-code', '2.0.9', false, 'release'),
      build('codex', '0.160.1', true, 'release'),
      build('acme-agent', '1.4.0', true, 'admin'),
    ];

    this.identityProviders = [
      {
        id: fixtureId(26, 1),
        key: 'acme-sso',
        name: 'Acme single sign-on',
        issuer: 'https://login.acme.example',
        client_id: 'orch',
        client_secret_set: true,
        scopes: ['openid', 'email', 'profile'],
        claim_subject: 'sub',
        claim_email: 'email',
        claim_name: 'name',
        claim_username: 'preferred_username',
        allowed_email_domains: ['acme.example'],
        auto_register: true,
        default_org_id: fixtureId(2, 100),
        default_role: 'developer',
        enabled: true,
        callback_url: `${APP_ORIGIN}/api/v1/auth/oidc/acme-sso/callback`,
        revision: 1,
      },
    ];
  }

  private root(orgId: string, slug: string, name: string, owner: UserRef, more: Partial<Org> = {}): Org {
    return {
      id: orgId,
      kind: 'org',
      org_kind: 'team',
      org_id: orgId,
      parent_id: null,
      depth: 0,
      slug,
      address: slug,
      name,
      description: '',
      readme: null,
      color: null,
      title_source: 'user',
      description_source: 'user',
      tags: [],
      tags_source: 'user',
      auto_updated_at: null,
      path: [{ id: orgId, kind: 'org', name, redacted: false }],
      owner,
      owners: 1,
      page: 'follows',
      access: { admin_only: [], participating: false, permissions: [], role: 'admin', role_from: 'system', administers: true, owns: false, subscription: null, effective_subscription: 'off' },
      archived: false,
      archived_at: null,
      archived_by: null,
      chat_id: fixtureId(9, 900),
      closed: false,
      flat: false,
      member_count: 1,
      org_count: 0,
      project_count: 0,
      changed_at: MADE_AT,
      revision: 1,
      created_at: MADE_AT,
      updated_at: MADE_AT,
      ...more,
    };
  }

  private agent(name: string, kind: string, profile: Agent['profile']): Agent {
    const created = new Date().toISOString();
    return {
      id: fixtureId(27, this.world.resources.agents.length + 1),
      name,
      address: name.toLowerCase().replaceAll(' ', '-'),
      harness_kind: kind, harness: harnessName(kind),
      home: null,
      org_id: null,
      owner: null,
      machine: null,
      disabled: false,
      profile,
      descriptor: {},
      descriptor_overridden: [],
      revision: 1,
      created_at: created,
      updated_at: created,
      access: { admin_only: [], participating: false, administers: true, permissions: [], through: [] },
    };
  }

  /** Puts the double back as it was made. */
  reset(): void {
    this.installation = { release: null, backup: null, backup_warning: true, backup_error: null };
    this.own.clear();
    this.entries = [];
    this.seenAt.clear();
    this.orgInvited.clear();
    this.deletedOrgs = [];
    this.extraOrgs = [];
    this.stored = true;
    this.seed();
  }

  // ---------------------------------------------------------------- the audit trail

  /** Writes a line, as the real system does for every act that changes something. */
  audit(by: Person | null, action: string, summary: string, details: unknown = {}): void {
    this.audited += 1;
    this.entries.unshift({
      id: fixtureId(28, 10_000 + this.audited),
      at: new Date().toISOString(),
      action,
      actor: by === null ? { kind: 'system' } : { kind: 'user', user: userRef(by.me) },
      as_admin: false,
      override: false,
      details,
      event_deletion_id: null,
      history: true,
      ip: '203.0.113.7',
      kind: kindOf(action) ?? 'changed',
      node: null,
      object: { ...LIVE_OBJECT, kind: action.split('.')[0] ?? 'object' },
      request_id: null,
      summary,
    });
  }

  // ---------------------------------------------------------------- one request

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>, _actKey?: string, requestId?: string): Answer | null {
    const route = `${method} ${path}`;
    let found: RegExpExecArray | null;

    // What the system's screens read that does not lie under /admin.
    if (route === 'GET /harness-kinds') return { status: 200, body: this.harnessKinds() };
    if (route === 'GET /pki/ca.pem') {
      // The authority's certificate is public: anybody may fetch it.
      return { status: 200, text: '-----BEGIN CERTIFICATE-----\nMIIBdouble\n-----END CERTIFICATE-----\n', headers: { 'Content-Disposition': 'attachment; filename="orch-ca.pem"' } };
    }
    if ((found = /^POST \/nodes\/([^/]+)\/restore$/.exec(route))) {
      const org = this.deletedOrgs.find((one) => one.id === found?.[1]);
      if (org === undefined) return null;
      this.need(person);
      this.deletedOrgs = this.deletedOrgs.filter((one) => one.id !== org.id);
      const restored = { ...org, archived: true, archived_at: new Date().toISOString() };
      this.extraOrgs.push(restored);
      this.audit(person, 'node.restored', `${person.me.display_name} restored ${org.name}`);
      return { status: 200, body: restored };
    }
    if ((found = /^POST \/nodes\/([^/]+)\/transfer$/.exec(route))) {
      const org = this.extraOrgs.find((one) => one.id === found?.[1]);
      if (org === undefined) return null;
      this.need(person);
      const next = [...this.world.people.values()].find((one) => one.me.id === text(body.user_id));
      if (next === undefined) invalid('user_id', 'is not a person of this system');
      const named = { ...org, owner: userRef(next.me), owners: org.owners + 1, revision: org.revision + 1 };
      this.extraOrgs = this.extraOrgs.map((one) => (one.id === org.id ? named : one));
      this.audit(person, 'node.owner_added', `${person.me.display_name} named ${named.owner.display_name} an owner of ${org.name}`);
      return { status: 200, body: named };
    }
    if ((found = /^GET \/orgs\/([^/]+)\/members$/.exec(route))) {
      const org = this.extraOrgs.find((one) => one.id === found?.[1]);
      if (org === undefined) return null;
      this.need(person);
      return { status: 200, body: this.peopleOfOrphans(org, query.get('q') ?? '') };
    }
    if (!path.startsWith('/admin/')) return null;
    this.need(person);

    if (route === 'GET /admin/system') return { status: 200, body: this.system() };
    if (route === 'GET /admin/semantic-search') return { status: 200, body: { available: true, unavailable_reason: null, interval_secs: 10, orgs: [] } };
    if (route === 'GET /admin/installation-records') return { status: 200, body: this.installation };
    if (route === 'GET /admin/backup-restore-list') return { status: 200, body: null };
    if (route === 'POST /admin/releases/check') {
      this.installation.release = { checked_at: new Date().toISOString(), installed_version: this.system().version, offered_version: null, newer: null, source: '', error: 'No release source is configured in this test installation.', command: null };
      return { status: 200, body: this.installation.release };
    }
    if (route === 'GET /admin/metrics') return { status: 200, body: this.metrics(query) };

    if (route === 'GET /admin/users') return { status: 200, body: this.userList(query) };
    if (route === 'POST /admin/users') return { status: 200, body: this.makeUser(person, body) };
    if ((found = /^PATCH \/admin\/users\/([^/]+)$/.exec(route))) return { status: 200, body: this.changeUser(person, found[1] ?? '', body) };
    if ((found = /^POST \/admin\/users\/([^/]+)\/reset-link$/.exec(route))) return { status: 200, body: this.resetLink(person, found[1] ?? '', body) };
    if ((found = /^(POST|DELETE) \/admin\/users\/([^/]+)\/org-invitation$/.exec(route))) return this.orgInvitation(person, found[2] ?? '', found[1] === 'POST');

    if (route === 'GET /admin/orgs') return { status: 200, body: this.orgList(person, query) };
    if ((found = /^PATCH \/admin\/orgs\/([^/]+)$/.exec(route))) return { status: 200, body: this.setLimits(person, found[1] ?? '', body) };

    if (route === 'GET /admin/settings') return { status: 200, body: this.settingList() };
    if ((found = /^(PUT|DELETE) \/admin\/settings\/([^/]+)$/.exec(route))) return { status: 200, body: this.setting(person, found[1] ?? '', found[2] ?? '', body, query, requestId) };

    if (route === 'GET /admin/hosts') return { status: 200, body: this.hosts() };
    if (route === 'GET /admin/runtimes') return { status: 200, body: this.runtimes(query) };
    if (route === 'GET /admin/storage') return { status: 200, body: this.storage() };
    if (route === 'GET /admin/pki') return { status: 200, body: this.pki() };
    if (route === 'GET /admin/plugins') return { status: 200, body: this.plugins() };

    if (route === 'GET /admin/capabilities') return { status: 200, body: this.capabilities };
    if ((found = /^PUT \/admin\/capabilities\/([^/]+)$/.exec(route))) return { status: 200, body: this.saveCapability(person, found[1] ?? '', body) };
    if ((found = /^DELETE \/admin\/capabilities\/([^/]+)$/.exec(route))) return { status: 200, body: this.deleteCapability(person, found[1] ?? '') };

    if (route === 'GET /admin/programs') {
      const q = (query.get('q') ?? '').toLowerCase();
      return { status: 200, body: pageOf(this.programSummaries().filter((p) => [p.harness.kind, p.harness.name, ...Object.values(p.harness.names ?? {})].some((name) => name.toLowerCase().includes(q))), query) };
    }
    if ((found = /^GET \/admin\/programs\/([^/]+)$/.exec(route))) return { status: 200, body: this.programDetail(decodeURIComponent(found[1] ?? '')) };
    if ((found = /^GET \/admin\/programs\/([^/]+)\/builds$/.exec(route))) return { status: 200, body: pageOf(this.builds.filter((b) => b.kind === decodeURIComponent(found![1] ?? '')).sort((a, b) => compareVersions(b.version, a.version)), query) };
    if ((found = /^GET \/admin\/programs\/([^/]+)\/builds\/([^/]+)$/.exec(route))) return { status: 200, body: this.builds.filter((b) => b.kind === decodeURIComponent(found![1] ?? '') && b.version === decodeURIComponent(found![2] ?? '')) };
    if ((found = /^PUT \/admin\/programs\/([^/]+)\/builds\/([^/]+)$/.exec(route))) {
      const kind = decodeURIComponent(found[1] ?? ''), version = decodeURIComponent(found[2] ?? '');
      this.saveBuild(person, kind, version, body);
      return { status: 200, body: this.builds.find((b) => b.kind === kind && b.version === version && b.os === body.os && b.arch === body.arch) };
    }
    if ((found = /^POST \/admin\/programs\/([^/]+)\/builds\/([^/]+)\/check$/.exec(route))) {
      this.checkBuild(decodeURIComponent(found[1] ?? ''), decodeURIComponent(found[2] ?? ''));
      return { status: 200, body: { queued: 1 } };
    }
    if ((found = /^POST \/admin\/programs\/([^/]+)\/builds\/([^/]+)\/fetch$/.exec(route))) return { status: 200, body: this.builds.filter((b) => b.kind === decodeURIComponent(found![1] ?? '') && b.version === decodeURIComponent(found![2] ?? '')) };
    if (/^POST \/admin\/programs\/[^/]+\/channels$/.test(route)) { this.channelsReadAt = new Date().toISOString(); return { status: 200, body: { kept: 0 } }; }
    if (route === 'GET /admin/harnesses') return { status: 200, body: this.builds };
    if (route === 'GET /admin/harnesses/channels') return { status: 200, body: this.channelStatus() };
    if ((found = /^PUT \/admin\/harnesses\/([^/]+)\/([^/]+)$/.exec(route))) return { status: 200, body: this.saveBuild(person, found[1] ?? '', found[2] ?? '', body) };
    // The fake reaches no vendor: a read of the channels keeps what it has.
    if (route === 'POST /admin/harnesses/channels') { this.channelsReadAt = new Date().toISOString(); return { status: 200, body: this.builds }; }
    if ((found = /^POST \/admin\/harnesses\/([^/]+)\/([^/]+)\/check$/.exec(route))) return { status: 200, body: this.checkBuild(found[1] ?? '', found[2] ?? '') };
    if (route === 'POST /admin/agents') return { status: 200, body: this.makeAgent(person, body) };

    if (route === 'GET /admin/audit') return { status: 200, body: this.auditList(query) };
    if (route === 'GET /admin/audit/export') return this.auditExport(person, query);

    if (route === 'GET /admin/identity-providers') return { status: 200, body: this.identityProviders };
    if (route === 'POST /admin/identity-providers') return { status: 200, body: this.makeProvider(person, body, requestId) };
    if ((found = /^PATCH \/admin\/identity-providers\/([^/]+)$/.exec(route))) return { status: 200, body: this.changeProvider(person, found[1] ?? '', body, requestId) };
    if ((found = /^DELETE \/admin\/identity-providers\/([^/]+)$/.exec(route))) return this.deleteProvider(person, found[1] ?? '', requestId);
    if ((found = /^POST \/admin\/identity-providers\/([^/]+)\/test$/.exec(route))) return this.testProvider(found[1] ?? '');
    // What another part of the world answers under /admin (the people and the trail of the system are the trace's).
    return null;
  }

  /** `/__double/admin/state`: the state of the parts of the system and whether anything was stored, as a test sets them. */
  control(path: string, body: Record<string, unknown>): Answer | null {
    if (path !== '/state') return null;
    const part = body.component as { name?: string; ok?: boolean; detail?: string | null; seen_at?: string | null } | undefined;
    if (part?.name !== undefined) {
      this.components = this.components.map((one) =>
        one.name === part.name ? { ...one, ok: part.ok ?? one.ok, detail: part.detail ?? one.detail, seen_at: part.seen_at === undefined ? one.seen_at : part.seen_at } : one,
      );
    }
    if (typeof body.stored === 'boolean') this.stored = body.stored;
    return { status: 204 };
  }

  /** Everything here is for a system admin, and a refusal to anybody else is the same whatever they asked. */
  private need(person: Person): void {
    if (person.me.is_system_admin) return;
    throw new Refusal(403, 'authz.denied', 'only a system admin may do this', { required: 'system.admin', requestable: false, override: false, missing: [] });
  }

  // ---------------------------------------------------------------- the state of the system

  system(): SystemInfo {
    return {
      version: '0.1.0',
      build: 'double 1',
      started_at: new Date(Date.now() - 3 * HOUR_MS).toISOString(),
      components: this.components,
      machines_online: 3,
      threads_running: 4,
      queues: { outbox_pending: 2, commands_open: 1, jobs_running: 0 },
    };
  }

  /** The values the system "stored": deterministic in the moment, so that a chart is the same on every look. */
  private metrics(query: URLSearchParams): MetricSeries[] {
    const names = query.getAll('name');
    const from = Date.parse(query.get('from') ?? '');
    const to = Date.parse(query.get('to') ?? '');
    if (Number.isNaN(from) || Number.isNaN(to)) invalid('from', 'is not a moment');
    if (to < from) invalid('to', 'must not be before from');
    const now = Date.now();
    const minutes = from >= now - METRICS_FINE_DAYS * DAY_MS;
    const allowed = minutes ? METRIC_STEPS : METRIC_STEPS.filter((step) => step >= 60);
    const points = (step: number) => Math.floor(to / (step * MINUTE_MS)) - Math.floor(from / (step * MINUTE_MS)) + 1;
    const asked = query.get('step');
    let step: number | undefined;
    if (asked !== null) {
      step = Number(asked);
      if (!METRIC_STEPS.includes(step)) invalid('step', 'is a number of minutes: 1, 5, 60 or 1440');
      if (!allowed.includes(step)) invalid('step', 'a span of that age is kept by the hour: ask for a step of 60 or 1440');
      if (points(step) > METRIC_MAX_POINTS) invalid('step', `gives more than ${METRIC_MAX_POINTS} points over this span`);
    } else {
      step = allowed.find((one) => points(one) <= METRIC_MAX_POINTS);
      if (step === undefined) invalid('from', `this span has more than ${METRIC_MAX_POINTS} points even at a step of a day: ask for a shorter one`);
    }
    const stepMs = step * MINUTE_MS;
    const resolution = minutes ? 'minute' : 'hour';
    const MEASURES: Record<string, { labels: Record<string, string>[]; value: (at: number, labels: Record<string, string>) => number }> = {
      threads: {
        labels: ['running', 'waiting_approval', 'blocked', 'idle', 'done'].map((state) => ({ state })),
        value: (at, { state }) => {
          const wave = Math.sin(at / (6 * HOUR_MS));
          return { running: 4 + Math.round(2 * wave), waiting_approval: 1 + Math.round(wave > 0.5 ? 1 : 0), blocked: wave < -0.9 ? 1 : 0, idle: 9, done: 20 + Math.round(at / (40 * DAY_MS)) % 7 }[state ?? ''] ?? 0;
        },
      },
      turns_total: {
        labels: [{ status: 'completed' }, { status: 'failed' }],
        value: (at, { status }) => (status === 'failed' ? Math.max(0, Math.round(1 + Math.sin(at / (3 * HOUR_MS)))) : 6 + Math.round(4 * Math.sin(at / (5 * HOUR_MS)))) * (stepMs / HOUR_MS > 1 ? Math.min(24, stepMs / HOUR_MS) : 1),
      },
      turn_seconds: { labels: [{}], value: (at) => 42 + 12 * Math.sin(at / (4 * HOUR_MS)) },
      link_sessions: { labels: [{ placement: 'connected' }, { placement: 'managed' }], value: (at, { placement }) => (placement === 'managed' ? 2 : 1 + Math.round(Math.abs(Math.sin(at / (9 * HOUR_MS))))) },
      outbox_pending: { labels: [{ channel: 'email' }], value: (at) => Math.max(0, Math.round(2 + 2 * Math.sin(at / (2 * HOUR_MS)))) },
      turns_waiting: { labels: [{ reason: 'cap' }, { reason: 'lock' }], value: (at, { reason }) => (reason === 'cap' ? Math.max(0, Math.round(Math.sin(at / (7 * HOUR_MS)) * 2)) : 0) },
      blob_store_bytes: { labels: [{ class: 'pinned' }, { class: 'cache' }], value: (at, { class: kind }) => (kind === 'pinned' ? 12 : 29 + (at % (9 * DAY_MS)) / DAY_MS) * 1024 ** 3 },
      blob_store_free_bytes: { labels: [{}], value: (at) => (210 - ((at % (9 * DAY_MS)) / DAY_MS)) * 1024 ** 3 },
    };
    if (names.length === 0) {
      return this.stored ? Object.keys(MEASURES).map((name): MetricSeries => ({ name, labels: {}, resolution, points: [], peaks: null })) : [];
    }
    if (!this.stored) return [];
    const series: MetricSeries[] = [];
    for (const name of names) {
      const measure = MEASURES[name];
      if (measure === undefined) continue;
      for (const labels of measure.labels) {
        const stamps: number[] = [];
        for (let at = Math.floor(from / stepMs) * stepMs; at <= to; at += stepMs) stamps.push(at);
        const values = stamps.map((at) => measure.value(at, labels));
        series.push({
          name,
          labels,
          resolution,
          points: stamps.map((at, index): [string, number] => [new Date(at).toISOString(), values[index] ?? 0]),
          peaks: minutes ? null : stamps.map((at, index): [string, number] => [new Date(at).toISOString(), (values[index] ?? 0) * 1.25]),
        });
      }
    }
    return series;
  }

  // ---------------------------------------------------------------- people

  adminUser(person: Person): AdminUser {
    const now = Date.now();
    const locked = (Object.entries(person.locked) as [keyof Person['locked'], number][])
      .filter(([, until]) => until > now)
      .map(([way, until]) => ({ way, until: new Date(until).toISOString() }));
    return {
      ...userRef(person.me),
      email: person.me.email,
      email_verified: person.me.email_verified,
      status: person.suspended ? 'suspended' : 'active',
      is_system_admin: person.me.is_system_admin,
      totp_enabled: person.me.totp_enabled,
      email_code_second_step: person.me.email_code_second_step,
      locked,
      org_invited_at: this.orgInvited.get(person.me.username) ?? null,
      created_at: person.me.created_at,
      last_seen_at: this.seenAt.get(person.me.username) ?? null,
    };
  }

  /** Whether `person` may make a team org now, by `orgs.created_by`: what the real server answers in the meta and holds to at the making. */
  mayMakeOrg(person: Person): boolean {
    // The meta mirrors the setting, and a scenario may set the meta alone: it is what the world holds.
    const makers = this.world.meta.org_creation;
    if (makers === 'anyone') return true;
    if (person.me.is_system_admin) return true;
    return makers === 'invited' && this.orgInvited.has(person.me.username);
  }

  /** Lets `person` make a team org where the policy does, using up an invitation that is what lets them; `false` where it does not. */
  takeOrgMaking(person: Person): boolean {
    if (!this.mayMakeOrg(person)) return false;
    if (this.world.meta.org_creation === 'invited' && !person.me.is_system_admin) this.orgInvited.delete(person.me.username);
    return true;
  }

  private orgInvitation(by: Person, id: string, give: boolean): Answer {
    const target = [...this.world.people.values()].find((one) => one.me.id === id);
    if (target === undefined) throw new Refusal(404, 'not_found', 'no such person');
    const held = this.orgInvited.has(target.me.username);
    if (give && !held) {
      this.orgInvited.set(target.me.username, new Date().toISOString());
      this.audit(by, 'org.invitation_given', `${by.me.display_name} invited ${target.me.username} to make a team org`);
      this.world.users.tell(target.me.username, { what: 'org_creation' });
    }
    if (!give && held) {
      this.orgInvited.delete(target.me.username);
      this.audit(by, 'org.invitation_taken_back', `${by.me.display_name} took back the invitation of ${target.me.username} to make a team org`);
      this.world.users.tell(target.me.username, { what: 'org_creation' });
    }
    return { status: 204 };
  }

  private userList(query: URLSearchParams) {
    const q = (query.get('q') ?? '').trim().toLowerCase();
    const status = query.get('status');
    const admins = query.get('system_admin'), after = query.get('cursor');
    const size = Math.min(PAGE_MAX, Math.max(1, Number(query.get('limit') ?? PAGE_DEFAULT)));
    // Native identity::people filters before its stable username COLLATE C cursor.
    // Usernames use the native ASCII alphabet; display names do not determine page positions.
    const all = [...this.world.people.values()]
      .filter((one) => one.me.kind === 'human')
      .filter((one) => q === '' || [one.me.username, one.me.display_name, one.me.email ?? ''].some((part) => part.toLowerCase().includes(q)))
      .filter((one) => status === null || (status === 'suspended') === one.suspended)
      .filter((one) => admins === null || (admins === 'true') === one.me.is_system_admin)
      .filter((one) => after === null || one.me.username > after)
      .sort((a, b) => a.me.username < b.me.username ? -1 : a.me.username > b.me.username ? 1 : 0);
    const items = all.slice(0, size).map((one) => this.adminUser(one));
    return { items, next_cursor: all.length > size ? items.at(-1)?.username ?? null : null };
  }

  private makeUser(by: Person, body: Record<string, unknown>) {
    const username = text(body.username);
    const display = text(body.display_name).trim();
    const email = text(body.email).trim();
    const errors: { path: string; message: string }[] = [];
    if (!USERNAME.test(username)) errors.push({ path: 'username', message: 'must be 3 to 32 of a-z, 0-9, - and _' });
    if (display === '') errors.push({ path: 'display_name', message: 'must not be empty' });
    if (email !== '' && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) errors.push({ path: 'email', message: 'is not an address' });
    if (errors.length > 0) throw new Refusal(400, 'request.invalid', 'the request is not valid', { errors });
    if ([...this.world.people.values()].some((one) => one.me.username === username || (email !== '' && one.me.email === email))) {
      throw new Refusal(409, 'conflict.exists', 'the username or the address is taken');
    }
    const made = this.world.addPerson(
      makeMe(this.world.people.size + 1, username, display, { email: email === '' ? null : email, email_verified: false, has_password: false, is_system_admin: flag(body.is_system_admin) }),
      randomUUID(),
    );
    const own = this.world.orgs.find((org) => org.id === made.me.personal_org_id);
    const policy = (body.personal_org ?? {}) as Record<string, unknown>;
    if (own !== undefined) {
      own.closed = policy.closed === undefined ? this.settingValue('orgs.personal_closed') !== false : flag(policy.closed);
      own.flat = policy.flat === undefined ? this.settingValue('orgs.personal_flat') !== false : flag(policy.flat);
    }
    this.audit(by, 'user.registered', `${by.me.display_name} made the user ${display}`);
    return { user: this.adminUser(made), reset_url: this.mintLink(made.me.username) };
  }

  /** A link at which a person sets a password, in the double's own table of them. */
  private mintLink(username: string): string {
    const token = `reset-${randomUUID().slice(0, 12)}`;
    this.world.resets.set(token, username);
    return `${APP_ORIGIN}/reset/${token}`;
  }

  private personById(id: string): Person {
    const found = [...this.world.people.values()].find((one) => one.me.id === id);
    if (found === undefined) throw new Refusal(404, 'not_found', 'no such person');
    return found;
  }

  private changeUser(by: Person, id: string, body: Record<string, unknown>): AdminUser {
    const target = this.personById(id);
    const wasAdmin = target.me.is_system_admin;
    const wants = body.is_system_admin;
    if ((wasAdmin || wants === true) && body.acknowledge_admin_risk !== true) {
      throw new Refusal(400, 'admin.risk_not_acknowledged', 'a change to a system admin must be acknowledged');
    }
    const demoting = wasAdmin && (wants === false || body.status === 'suspended');
    if (demoting) {
      const others = [...this.world.people.values()].filter((one) => one !== target && one.me.is_system_admin && !one.suspended);
      if (others.length === 0) throw new Refusal(409, 'admin.last_admin', 'the last system admin cannot be demoted or suspended');
    }
    if (body.status === 'active' && target.suspended) {
      target.suspended = false;
      this.audit(by, 'user.resumed', `${by.me.display_name} resumed ${target.me.display_name}`);
    }
    if (wants === true && !wasAdmin) {
      target.me = { ...target.me, is_system_admin: true };
      this.audit(by, 'user.admin_granted', `${by.me.display_name} made ${target.me.display_name} a system admin`);
    }
    if (wants === false && wasAdmin) {
      target.me = { ...target.me, is_system_admin: false };
      this.audit(by, 'user.admin_revoked', `${by.me.display_name} took the system admin role from ${target.me.display_name}`);
    }
    if (body.status === 'suspended' && !target.suspended) {
      target.suspended = true;
      this.audit(by, 'user.suspended', `${by.me.display_name} suspended ${target.me.display_name}`);
    }
    return this.adminUser(target);
  }

  private resetLink(by: Person, id: string, body: Record<string, unknown>) {
    const target = this.personById(id);
    this.audit(by, 'user.reset_link_made', `${by.me.display_name} made a reset link for ${target.me.display_name}`, { clear_second_step: flag(body.clear_second_step) });
    return { url: this.mintLink(target.me.username), expires_at: new Date(Date.now() + 24 * HOUR_MS).toISOString() };
  }

  // ---------------------------------------------------------------- root orgs

  private roots(): Org[] {
    return [...this.world.orgs.filter((org) => org.depth === 0), ...this.extraOrgs];
  }

  private orgList(person: Person, query: URLSearchParams) {
    const q = (query.get('q') ?? '').trim().toLowerCase();
    const kind = query.get('org_kind');
    // The deleted fixture flow is separate from the active name/cursor contract.
    if (query.get('deleted') === 'only') return pageOf(this.deletedOrgs
      .filter((org) => q === '' || org.name.toLowerCase().includes(q) || org.slug.toLowerCase().includes(q))
      .filter((org) => kind === null || org.org_kind === kind)
      .sort((a, b) => Number(a.org_kind === 'personal') - Number(b.org_kind === 'personal') || a.name.localeCompare(b.name)), query);

    // Native hierarchy::orgs_by_name includes departments. Read their ordinary
    // authorized DTOs; the existing route guard still requires admin mode.
    const departments = [...this.world.trees.values()].flatMap((tree) => [...tree.nodes.values()])
      .filter((row) => row.kind === 'org' && row.parent_id !== null && !row.deleted && this.world.places.visibleTo(person, row))
      .map((row) => this.world.places.org(person, row));
    interface Position { personal: boolean; id: string; name: string }
    const position = (org: Org): Position => ({ personal: org.org_kind === 'personal', id: org.id, name: org.name });
    // PostgreSQL lower(name) COLLATE "C" compares UTF-8 bytes, not localeCompare
    // or UTF-16 code units; ID is the stable tie break for equal folded names.
    const compare = (a: Position, b: Position) => Number(a.personal) - Number(b.personal)
      || Buffer.compare(Buffer.from(a.name.toLowerCase()), Buffer.from(b.name.toLowerCase()))
      || Buffer.compare(Buffer.from(a.id), Buffer.from(b.id));
    const cursor = query.get('cursor');
    let after: Position | null = null;
    if (cursor !== null) {
      const parts = /^([pt])\|([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})\|([\s\S]*)$/u.exec(cursor);
      if (parts === null) invalid('cursor', 'is not a cursor of this list');
      after = { personal: parts[1] === 'p', id: (parts[2] ?? '').toLowerCase(), name: parts[3] ?? '' };
    }
    const all = [...new Map([...this.roots(), ...departments].map((org) => [org.id, org])).values()]
      .filter((org) => (q === '' || org.name.toLowerCase().includes(q)) && (kind === null || org.org_kind === kind))
      .filter((org) => after === null || compare(position(org), after) > 0)
      .sort((a, b) => compare(position(a), position(b)));
    const asked = query.get('limit');
    if (asked !== null && (!/^\d+$/u.test(asked) || Number(asked) > 0xffff_ffff)) invalid('limit', 'is not an unsigned 32-bit page size');
    if (kind !== null && kind !== 'team' && kind !== 'personal') invalid('org_kind', 'is not an organization kind');
    const size = Math.min(PAGE_MAX, Math.max(1, Number(asked ?? PAGE_DEFAULT)));
    const items = all.slice(0, size), last = items.at(-1);
    return { items, next_cursor: all.length > size && last !== undefined ? `${last.org_kind === 'personal' ? 'p' : 't'}|${last.id}|${last.name}` : null };
  }

  private setLimits(by: Person, id: string, body: Record<string, unknown>): Org {
    const org = this.roots().find((one) => one.id === id);
    if (org === undefined) throw new Refusal(400, 'request.invalid', 'that org lies in another', { errors: [{ path: 'id', message: 'is not a root org' }] });
    const changes: string[] = [];
    if (typeof body.closed === 'boolean') {
      org.closed = body.closed;
      changes.push(body.closed ? 'closed to further members' : 'opened to further members');
    }
    if (typeof body.flat === 'boolean') {
      org.flat = body.flat;
      changes.push(body.flat ? 'stopped from holding orgs inside it' : 'allowed to hold orgs inside it');
    }
    org.revision += 1;
    this.audit(by, 'org.limits_changed', `${by.me.display_name} ${changes.join(' and ')}: ${org.name}`, { closed: org.closed, flat: org.flat });
    return org;
  }

  private peopleOfOrphans(org: Org, q: string) {
    const needle = q.trim().toLowerCase();
    const items = [...this.world.people.values()]
      .filter((one) => needle === '' || one.me.display_name.toLowerCase().includes(needle) || one.me.username.includes(needle))
      .slice(0, 8)
      .map((one) => ({
        address: org.address,
        also: 0,
        domain: { id: org.id, kind: 'org' as const, name: org.name, redacted: false },
        is_owner: false,
        role: 'developer' as const,
        since: MADE_AT,
        suspended: one.suspended,
        user: userRef(one.me),
        second_step: null,
        second_step_required: null,
        last_seen_at: null,
      }));
    return { items, next_cursor: null };
  }

  // ---------------------------------------------------------------- settings

  /** Whether somebody who registers gets a personal org, and how it is made, as the system's settings say. */
  personalHome(): { enabled: boolean; closed: boolean; flat: boolean } {
    return {
      enabled: this.settingValue('orgs.personal_enabled') !== false,
      closed: this.settingValue('orgs.personal_closed') !== false,
      flat: this.settingValue('orgs.personal_flat') !== false,
    };
  }

  settingValue(key: string): unknown {
    const own = this.own.get(key);
    return own === undefined ? SETTING_DEFAULTS[key] : own.value;
  }

  private channelStatus(): HarnessChannelsStatus {
    const value = this.settingValue('agents.harness_versions');
    const choices = typeof value === 'object' && value !== null && !Array.isArray(value) ? Object.entries(value as Record<string, unknown>) : [];
    const requested: string[] = [];
    let pinned = false;
    for (const [kind, choice] of choices) {
      const use = typeof choice === 'object' && choice !== null ? (choice as Record<string, unknown>).use : undefined;
      if (use === 'newest' || use === 'newest_checked') requested.push(kind);
      if (use === 'exact') pinned = true;
    }
    const interval = this.settingValue('agents.harness_channel_hours');
    const interval_hours = typeof interval === 'number' ? interval : 24;
    const reason = interval_hours === 0 ? 'disabled' : requested.length === 0 ? pinned ? 'pinned' : 'shipped' : requested.some((kind) => kind === 'codex' || kind === 'claude-code') ? 'newer' : 'no_channel';
    return { last_read_at: this.channelsReadAt, reason, interval_hours, requested };
  }

  private resolved(key: string): ResolvedSetting {
    const own = this.own.get(key);
    return {
      key,
      value: this.settingValue(key),
      source: { scope: own === undefined ? 'default' : 'system', node: null },
      own: own === undefined ? null : { value: own.value, locked: own.locked, revision: own.revision },
      locked_by: null,
      writable: true,
      // A setting of the system has no place above it: nothing is supplied but its own part.
      parts: [],
    };
  }

  private settingList(): ResolvedSetting[] {
    return Object.keys(SETTING_DEFAULTS).map((key) => this.resolved(key));
  }

  private setting(by: Person, method: string, key: string, body: Record<string, unknown>, query: URLSearchParams, requestId?: string): ResolvedSetting {
    if (!(key in SETTING_DEFAULTS)) throw new Refusal(404, 'not_found', `no setting ${key}`);
    const before = this.own.get(key);
    const raw = query.get('expected_revision');
    if (method === 'DELETE' && raw !== null && (!/^\d+$/.test(raw) || !Number.isSafeInteger(Number(raw)))) invalid('expected_revision', 'is a revision');
    const expected = method === 'DELETE' ? raw === null ? undefined : Number(raw) : typeof body.expected_revision === 'number' ? body.expected_revision : undefined;
    if (expected !== undefined && (method === 'DELETE' ? before?.revision !== expected : (before?.revision ?? 0) !== expected)) {
      throw new Refusal(409, 'conflict.stale_revision', 'the setting was changed meanwhile', { current: this.resolved(key) });
    }
    if (method === 'DELETE') {
      this.own.delete(key);
      this.audit(by, 'setting.cleared', `${by.me.display_name} put ${key} back to what Orch ships`);
    } else {
      const value = body.value;
      if (key === 'hosts.full_percent' && !(typeof value === 'number' && Number.isInteger(value) && value >= 1 && value <= 100)) invalid('value', 'must be a whole number from 1 to 100');
      if (key === 'hosts.when_full' && value !== 'warn' && value !== 'block') invalid('value', 'must be warn or block');
      if (key === 'registration.mode' && value !== 'closed' && value !== 'invite' && value !== 'open') invalid('value', 'must be closed, invite or open');
      if (key === 'registration.email_verification' && value !== 'off' && value !== 'required') invalid('value', 'must be off or required');
      if (key === 'system.banner' && value !== null) {
        const banner = value as Record<string, unknown> | undefined;
        if (typeof banner?.text !== 'string' || banner.text.trim() === '') invalid('value.text', 'must not be empty');
      }
      const locked = typeof body.locked === 'boolean' ? body.locked : before?.locked ?? false;
      if (before === undefined || JSON.stringify(before.value) !== JSON.stringify(value) || before.locked !== locked) {
        this.own.set(key, { value, locked, revision: (before?.revision ?? 0) + 1 });
        this.audit(by, 'setting.changed', `${by.me.display_name} changed the setting ${key}`);
      }
    }
    this.mirror(key);
    const after = this.own.get(key);
    if (JSON.stringify(before) !== JSON.stringify(after) && ['system.banner', 'storage.max_blob_bytes', 'registration.mode', 'orgs.created_by', 'auth.local_login', 'auth.email_code'].includes(key)) {
      this.world.users.tellAll({ what: 'meta_changed', ...(requestId === undefined ? {} : { request_id: requestId }) });
    }
    return this.resolved(key);
  }

  /** What the system says of itself to anybody follows the settings it is made from. */
  private mirror(key: string): void {
    const value = this.settingValue(key);
    if (key === 'usage.catalogue') this.world.usage.estimates = value === true;
    if (key === 'usage.catalogue_scale' && typeof value === 'string') this.world.usage.scale = value;
    if (key === 'usage.rates' && typeof value === 'object' && value !== null) this.world.usage.card = value as typeof this.world.usage.card;
    if (key === 'system.banner') this.world.meta = { ...this.world.meta, banner: value as typeof this.world.meta.banner };
    if (key === 'registration.mode') this.world.meta = { ...this.world.meta, registration_mode: value as typeof this.world.meta.registration_mode };
    if (key === 'orgs.created_by') this.world.meta = { ...this.world.meta, org_creation: value as typeof this.world.meta.org_creation };
    if (key === 'auth.local_login') this.world.meta = { ...this.world.meta, local_login: value !== false };
    if (key === 'auth.email_code') this.world.meta = { ...this.world.meta, email_code_login: Array.isArray(value) && value.includes('sign_in') };
  }

  // ---------------------------------------------------------------- the rest of the system

  private machine(name: string, online = true): MachineRef {
    return { id: fixtureId(22, name.length * 7), name, online, placement: 'managed', platform_os: 'linux' };
  }

  hosts(): Host[] {
    const systemWhenFull = this.settingValue('hosts.when_full') === 'block' ? 'block' : 'warn';
    const ada = this.world.people.get('ada');
    const vera = this.world.people.get('vera');
    const host = (n: number, name: string, memory: number, declared: number, more: Partial<Host> = {}): Host => ({
      access: { admin_only: [], participating: false, administers: true, permissions: [], through: [] },
      address: name,
      capacity: { cpu_millis: 16_000, disk_gb: 500, memory_mb: memory },
      declared: { cpu_millis: Math.round((declared / memory) * 16_000), disk_gb: 120, memory_mb: declared },
      drivers: [
        { creates: ['environment', 'provider'], name: 'docker', runtime_classes: ['container'], title: 'Docker' },
        { creates: ['environment'], name: 'host', runtime_classes: ['directory'], title: 'A directory on the host' },
      ],
      home: null,
      id: fixtureId(20, n),
      labels: ['linux'],
      machine: this.machine(name),
      mine: 0,
      name,
      org_id: null,
      owner: ada === undefined ? null : userRef(ada.me),
      ports: null,
      ports_proposal: null,
      ports_revision: 0,
      revision: 1,
      running: { environment: 2, provider: 1 },
      schedulable: true,
      storages: [{ capacity_gb: 500, default: true, free_gb: 380, name: 'local', private_views: true }],
      when_full: { own: false, value: systemWhenFull },
      ...more,
    });
    return [
      host(1, 'main', 65_536, 24_576),
      host(2, 'build-box', 32_768, 30_720, { running: { environment: 4, provider: 0 }, labels: ['linux', 'builds-images'] }),
      host(3, 'lab-gpu', 131_072, 126_976, {
        home: { id: fixtureId(2, 100), kind: 'org', name: 'Acme', redacted: false },
        org_id: fixtureId(2, 100),
        owner: vera === undefined ? null : userRef(vera.me),
        when_full: { own: true, value: 'block' },
        running: { environment: 6, provider: 0 },
        labels: ['linux', 'gpu'],
        machine: this.machine('lab-gpu'),
      }),
    ];
  }

  private runtimes(query: URLSearchParams): Runtime[] {
    const ada = this.world.people.get('ada');
    const owner = ada === undefined ? userRef(makeMe(1, 'ada', 'Ada Lovelace')) : userRef(ada.me);
    const hosts = this.hosts();
    const row = (n: number, kind: 'environment' | 'provider', host: Host, name: string, state: string, more: Partial<Runtime> = {}): Runtime => ({
      id: fixtureId(21, n),
      kind,
      host_id: host.id,
      driver: 'docker',
      unit_name: `orch-${kind === 'environment' ? 'env' : 'prov'}-${n}`,
      state,
      state_detail: null,
      generation: 1,
      machine: host.machine,
      network: { mode: 'open', required_mode: 'open', network_mode: 'bridge', networks: [], proxy_url: null, allowed_hosts: [], sources: [], revision: '', always: [], always_revision: '', always_source: 'orch_configuration', state: 'ready', reason: null, last_denied: null },
      template: { id: fixtureId(23, kind === 'environment' ? 1 : 2), name: kind === 'environment' ? 'Ubuntu with tools' : 'Provider runtime' },
      updates: 0,
      successor_of: null,
      remove_at: null,
      environment: kind === 'environment' ? { address: name.toLowerCase(), id: fixtureId(24, n), name, owner, placement: 'managed', status: 'running', ports_exposed: false } : null,
      provider:
        kind === 'provider'
          ? { address: name.toLowerCase(), harness_kind: 'claude-code', harness: harnessName('claude-code'), id: fixtureId(25, n), kind: 'subscription', name, placement: 'managed', status: 'ready' }
          : null,
      last_active_at: new Date(Date.now() - n * HOUR_MS).toISOString(),
      ...more,
    });
    const [main, box, lab] = hosts;
    if (main === undefined || box === undefined || lab === undefined) return [];
    const all = [
      row(1, 'environment', main, 'Main', 'running'),
      row(2, 'environment', main, 'Scratch', 'stopped', { last_active_at: null }),
      row(3, 'provider', main, 'Claude Max runtime', 'running', { updates: 2 }),
      row(4, 'environment', box, 'Build cache', 'running', { updates: 1 }),
      row(5, 'environment', lab, 'GPU lab', 'failed', { state_detail: 'The image could not be pulled: unauthorized' }),
      row(6, 'environment', lab, 'GPU lab (next)', 'provisioning', { successor_of: fixtureId(21, 5) }),
    ];
    const host = query.get('host_id');
    const kind = query.get('kind');
    return all.filter((one) => (host === null || one.host_id === host) && (kind === null || one.kind === kind));
  }

  private storage(): StorageStats {
    const ada = this.world.people.get('ada');
    const owner = ada === undefined ? userRef(makeMe(1, 'ada', 'Ada Lovelace')) : userRef(ada.me);
    const environment = (n: number, name: string) => ({ address: name.toLowerCase(), id: fixtureId(24, n), name, owner, placement: 'managed' as const, status: 'running' as const, ports_exposed: false });
    const gib = 1024 ** 3;
    return {
      blob_count: 18_432,
      blob_bytes: 41 * gib,
      pinned_bytes: 12 * gib,
      cache_bytes: 29 * gib,
      disk_free_bytes: 210 * gib,
      top_environments: [
        { environment: environment(1, 'Main'), cache_bytes: 11 * gib, pinned_bytes: 3 * gib },
        { environment: environment(4, 'Build cache'), cache_bytes: 9 * gib, pinned_bytes: 1 * gib },
        { environment: environment(5, 'GPU lab'), cache_bytes: 4 * gib, pinned_bytes: 6 * gib },
      ],
    };
  }

  private pki(): PkiInfo {
    const before = Date.now() + PKI_MACHINE_WARNING_DAYS * DAY_MS;
    const expiring = this.world.resources.machineCertificates()
      .filter((entry) => Date.parse(entry.not_after) <= before)
      .sort((a, b) => a.not_after.localeCompare(b.not_after) || a.machine.id.localeCompare(b.machine.id));
    return {
      ca: {
        subject: 'CN=Orch authority, O=Acme',
        not_before: '2026-01-01T00:00:00.000Z',
        not_after: '2036-01-01T00:00:00.000Z',
        fingerprint_sha256: 'a3f1c2d4e5b60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90',
      },
      services: [
        { name: 'api', not_after: '2027-01-01T00:00:00.000Z' },
        { name: 'gateway', not_after: '2026-10-25T00:00:00.000Z' },
        { name: 'worker', not_after: '2027-01-01T00:00:00.000Z' },
      ],
      machines_expiring_30d: expiring.length,
      machines_expiring: expiring.slice(0, PKI_MACHINES_LIMIT),
    };
  }

  private plugins(): AdminPlugin[] {
    return [
      { component: 'api', machine: null, kind: 'notifier', name: 'matrix', version: '0.3.1', state: 'running', detail: '' },
      { component: 'node', machine: this.machine('main'), kind: 'driver', name: 'podman', version: '1.2.0', state: 'restarting', detail: 'Restarting after it exited with code 2.' },
      { component: 'connector', machine: this.machine('vera-laptop', false), kind: 'executor', name: 'sandbox-x', version: '0.9.0', state: 'failed', detail: 'Protocol 3 is not supported by this release.' },
    ];
  }

  // ---------------------------------------------------------------- capabilities, harnesses, agents

  private saveCapability(by: Person, key: string, body: Record<string, unknown>): CapabilityDef {
    const before = this.capabilities.find((one) => one.key === key);
    if (before !== undefined && body.revision !== before.revision) throw new Refusal(409, 'conflict.stale_revision', 'the definition was changed by somebody else');
    if (!/^[a-z][a-z0-9_]{1,31}$/.test(key)) invalid('key', 'must be 2 to 32 of a-z, 0-9 and _');
    if (text(body.label).trim() === '') invalid('label', 'must not be empty');
    const def: CapabilityDef = {
      key,
      label: text(body.label).trim(),
      description: text(body.description),
      risk: (['low', 'medium', 'high', 'critical'] as const).find((one) => one === body.risk) ?? 'medium',
      default_decision: (['allow', 'ask', 'deny'] as const).find((one) => one === body.default_decision) ?? 'ask',
      default_approver_scope: (['host', 'org', 'system'] as const).find((one) => one === body.default_approver_scope) ?? 'host',
      driver_patch: (typeof body.driver_patch === 'object' && body.driver_patch !== null ? body.driver_patch : {}) as Record<string, unknown>,
      builtin: before?.builtin ?? false,
      revision: (before?.revision ?? 0) + 1,
    };
    this.capabilities = before === undefined ? [...this.capabilities, def] : this.capabilities.map((one) => (one.key === key ? def : one));
    this.audit(by, 'capability.defined', `${by.me.display_name} ${before === undefined ? 'defined' : 'changed'} the capability ${def.label}`);
    return def;
  }

  private deleteCapability(by: Person, key: string): CapabilityDef {
    const found = this.capabilities.find((one) => one.key === key);
    if (found === undefined) throw new Refusal(404, 'not_found', 'no such capability');
    if (found.builtin) invalid('key', 'ships with Orch and cannot be removed');
    this.capabilities = this.capabilities.filter((one) => one.key !== key);
    this.audit(by, 'capability.removed', `${by.me.display_name} removed the capability ${found.label}`);
    return found;
  }

  private harnessKinds(): HarnessKindInfo[] {
    const kinds: [string, string, Record<string, unknown>][] = [
      [
        'claude-code',
        'Claude Code',
        { kind: 'claude-code', harness: { name: 'Claude Code' }, capabilities: { steer: true, input_images: true, fork_native: true, phases: true, memory: 'directory', agent_terminal: 'view' } },
      ],
      [
        'codex',
        'Codex',
        { kind: 'codex', harness: { name: 'Codex' }, capabilities: { steer: true, input_images: true, fork_native: true, phases: false, memory: 'home', agent_terminal: 'none' } },
      ],
      [
        'acme-agent',
        'Acme agent',
        { kind: 'acme-agent', harness: { name: 'Acme agent' }, capabilities: { steer: false, input_images: false, fork_native: false, memory: 'none', agent_terminal: 'interactive' } },
      ],
    ];
    return kinds.map(([kind, title, descriptor]): HarnessKindInfo => ({
      kind,
      title,
      harness: harnessName(kind),
      program: null,
      descriptor: partialDescriptor(descriptor),
      builds: this.builds.filter((one) => one.kind === kind).map((one) => ({ version: one.version, is_default: one.is_default })),
    }));
  }

  private programSummaries(): ProgramSummary[] {
    return this.harnessKinds().map((info) => {
      const rows = this.builds.filter((b) => b.kind === info.kind).sort((a, b) => compareVersions(b.version, a.version));
      const platforms = [...new Set(rows.map((b) => `${b.os}/${b.arch}`))].map((platform) => {
        const builds = rows.filter((b) => `${b.os}/${b.arch}` === platform), first = builds[0]!;
        return { os: first.os, arch: first.arch, builds: builds.length, shipped: builds.find((b) => b.source === 'release')?.version ?? null,
          newest: first.version, newest_checked: builds.find((b) => b.status === 'checked')?.version ?? null, default_version: builds.find((b) => b.is_default)?.version ?? null };
      });
      return { harness: info.harness, builtin: info.kind !== 'acme-agent', protocol: info.kind === 'acme-agent' ? 'ACP' : 'native', profiles: 0, machines: 0,
        platforms, channel: this.channelsReadAt === null ? 'unread' : 'ready', check: rows[0]?.status ?? null };
    });
  }
  private programDetail(kind: string): ProgramDetail {
    const summary = this.programSummaries().find((p) => p.harness.kind === kind);
    const info = this.harnessKinds().find((p) => p.kind === kind);
    if (summary === undefined || info === undefined) throw new Refusal(404, 'not_found', 'no such program');
    return { summary, resolved_builds: this.builds.filter((b) => b.kind === kind && b.is_default), program: info.program ?? null, descriptor: info.descriptor, capabilities: info.descriptor === null ? null : [
      { id: 'conversation', title: { en: 'Conversation', 'zh-CN': '对话' }, items: [{ id: 'steer', title: { en: 'Steering', 'zh-CN': '引导' }, available: info.descriptor.capabilities.steer, reason: { en: 'This build does not declare steering', 'zh-CN': '此版本未声明引导能力' } }] },
      { id: 'tools', title: { en: 'Tools', 'zh-CN': '工具' }, items: [] },
      { id: 'sessions', title: { en: 'Sessions', 'zh-CN': '会话' }, items: [] },
      { id: 'usage', title: { en: 'Usage', 'zh-CN': '用量' }, items: [] },
    ], versions: this.settingList().find((s) => s.key === 'agents.harness_versions')?.value ?? {}, channels: this.channelStatus() };
  }

  private saveBuild(by: Person, kind: string, version: string, body: Record<string, unknown>): HarnessBuild[] {
    if (text(body.url) !== '' && !text(body.url).startsWith('https://')) invalid('url', 'must be an https address');
    if (!/^[0-9a-f]{64}$/.test(text(body.sha256))) invalid('sha256', 'must be 64 hexadecimal characters');
    const made = build(kind, version, flag(body.is_default), 'admin', text(body.url), text(body.sha256), text(body.os) === 'macos' ? 'macos' : text(body.os) === 'windows' ? 'windows' : 'linux', text(body.arch) === 'aarch64' ? 'aarch64' : 'x86_64');
    const others = this.builds.filter((one) => !(one.kind === kind && one.version === version && one.os === made.os && one.arch === made.arch));
    this.builds = [...(made.is_default ? others.map((one) => (one.kind === kind && one.os === made.os && one.arch === made.arch ? { ...one, is_default: false } : one)) : others), made];
    this.audit(by, 'harness.build_saved', `${by.me.display_name} saved the build ${kind} ${version}`);
    return this.builds;
  }

  /** A check asked for: the kept builds of that version are being checked, and pass at once in this world. */
  private checkBuild(kind: string, version: string): HarnessBuild[] {
    const kept = this.builds.filter((one) => one.kind === kind && one.version === version && one.source !== 'release');
    if (kept.length === 0) throw new Refusal(404, 'not_found', 'no such build');
    this.builds = this.builds.map((one) => (kept.includes(one) ? { ...one, status: 'checked', check: { at: new Date().toISOString(), machine: 'host-1', missing: [], off: [], error: null, stage: 'complete', cases: [] } } : one));
    return this.builds;
  }

  private makeAgent(by: Person, body: Record<string, unknown>) {
    if (text(body.name).trim() === '') invalid('name', 'must not be empty');
    if (text(body.harness_kind) === '') invalid('harness_kind', 'is required');
    const made = this.agent(text(body.name).trim(), text(body.harness_kind), body.profile ?? {});
    // The resources part keeps the one list of profiles that every list and picker reads.
    this.world.resources.agents.push({ id: made.id, name: made.name, harness: made.harness_kind, owner: null, home: null, machine: null, disabled: false, revision: 1, profile: made.profile as Record<string, unknown>, descriptor: {} });
    this.audit(by, 'agent.created', `${by.me.display_name} made the agent profile ${text(body.name)}`);
    return made;
  }

  // ---------------------------------------------------------------- audit

  private filteredEntries(query: URLSearchParams): AuditEntry[] {
    const actor = query.get('actor');
    const action = query.get('action');
    const from = query.get('from') === null ? null : Date.parse(query.get('from') ?? '');
    const to = query.get('to') === null ? null : Date.parse(query.get('to') ?? '');
    return this.entries.filter(
      (entry) =>
        (actor === null || (entry.actor.kind === 'user' && entry.actor.user.id === actor)) &&
        (action === null || entry.action === action) &&
        (from === null || Date.parse(entry.at) >= from) &&
        (to === null || Date.parse(entry.at) <= to),
    );
  }

  private auditList(query: URLSearchParams) {
    return pageOf(this.filteredEntries(query), query);
  }

  private auditExport(by: Person, query: URLSearchParams): Answer {
    const format = query.get('format');
    if (format !== 'csv' && format !== 'jsonl') invalid('format', 'is csv or jsonl');
    const from = Date.parse(query.get('from') ?? '');
    const to = Date.parse(query.get('to') ?? '');
    if (Number.isNaN(from) || Number.isNaN(to)) invalid('from', 'is not a moment');
    if (to - from > 366 * DAY_MS) invalid('to', 'is more than 366 days after from');
    this.audit(by, 'audit.exported', `${by.me.display_name} exported the audit trail`, { format, from: query.get('from'), to: query.get('to') });
    const rows = this.filteredEntries(new URLSearchParams({ from: query.get('from') ?? '', to: query.get('to') ?? '' })).slice().reverse();
    const cell = (value: string): string => (/[",\n]/.test(value) || /^[=+\-@]/.test(value) ? `"${value.replaceAll('"', '""').replace(/^([=+\-@])/, "'$1")}"` : value);
    const out =
      format === 'jsonl'
        ? rows.map((row) => JSON.stringify(row)).join('\n') + '\n'
        : ['at,actor,action,summary', ...rows.map((row) => [row.at, row.actor.kind === 'user' ? row.actor.user.username : row.actor.kind, row.action, row.summary].map(cell).join(','))].join('\n') + '\n';
    return {
      status: 200,
      text: out,
      headers: { 'Content-Type': format === 'csv' ? 'text/csv; charset=utf-8' : 'application/x-ndjson', 'Content-Disposition': `attachment; filename="audit.${format}"` },
    };
  }

  // ---------------------------------------------------------------- identity providers

  private makeProvider(by: Person, body: Record<string, unknown>, requestId?: string): IdentityProvider {
    const key = text(body.key);
    if (!/^[a-z][a-z0-9-]{1,31}$/.test(key)) invalid('key', 'must be 2 to 32 of a-z, 0-9 and -');
    if (!/^https:\/\/[^?#\s]+$/.test(text(body.issuer))) invalid('issuer', 'must be an https address without a query or a fragment');
    if (text(body.client_id) === '') invalid('client_id', 'must not be empty');
    if (this.identityProviders.some((one) => one.key === key)) throw new Refusal(409, 'conflict.exists', 'a provider has that key');
    const made: IdentityProvider = {
      id: fixtureId(26, this.identityProviders.length + 2),
      key,
      name: text(body.name) || key,
      issuer: text(body.issuer),
      client_id: text(body.client_id),
      client_secret_set: text(body.client_secret) !== '',
      scopes: Array.isArray(body.scopes) ? (body.scopes as string[]) : ['openid', 'email', 'profile'],
      claim_subject: text(body.claim_subject) || 'sub',
      claim_email: text(body.claim_email) || 'email',
      claim_name: text(body.claim_name) || 'name',
      claim_username: text(body.claim_username) || 'preferred_username',
      allowed_email_domains: Array.isArray(body.allowed_email_domains) ? (body.allowed_email_domains as string[]) : [],
      auto_register: flag(body.auto_register),
      default_org_id: typeof body.default_org_id === 'string' ? body.default_org_id : null,
      default_role: (['viewer', 'interactor', 'developer', 'admin'] as const).find((one) => one === body.default_role) ?? null,
      enabled: body.enabled !== false,
      callback_url: `${APP_ORIGIN}/api/v1/auth/oidc/${key}/callback`,
      revision: 1,
    };
    this.identityProviders = [...this.identityProviders, made];
    this.audit(by, 'identity_provider.created', `${by.me.display_name} added the identity provider ${made.name}`);
    this.world.users.tellAll({ what: 'meta_changed', ...(requestId === undefined ? {} : { request_id: requestId }) });
    return made;
  }

  private changeProvider(by: Person, id: string, body: Record<string, unknown>, requestId?: string): IdentityProvider {
    const before = this.identityProviders.find((one) => one.id === id);
    if (before === undefined) throw new Refusal(404, 'not_found', 'no such identity provider');
    if (body.expected_revision !== before.revision) throw new Refusal(409, 'conflict.stale_revision', 'the provider was changed by somebody else');
    if (typeof body.issuer === 'string' && !/^https:\/\/[^?#\s]+$/.test(body.issuer)) invalid('issuer', 'must be an https address without a query or a fragment');
    const made: IdentityProvider = {
      ...before,
      ...(typeof body.name === 'string' ? { name: body.name } : {}),
      ...(typeof body.issuer === 'string' ? { issuer: body.issuer } : {}),
      ...(typeof body.client_id === 'string' ? { client_id: body.client_id } : {}),
      ...(typeof body.client_secret === 'string' && body.client_secret !== '' ? { client_secret_set: true } : {}),
      ...(Array.isArray(body.scopes) ? { scopes: body.scopes as string[] } : {}),
      ...(Array.isArray(body.allowed_email_domains) ? { allowed_email_domains: body.allowed_email_domains as string[] } : {}),
      ...(typeof body.auto_register === 'boolean' ? { auto_register: body.auto_register } : {}),
      ...(typeof body.enabled === 'boolean' ? { enabled: body.enabled } : {}),
      ...(body.default_org_id === null || typeof body.default_org_id === 'string' ? { default_org_id: body.default_org_id } : {}),
      ...(body.default_role === null ? { default_role: null }
        : (['viewer', 'interactor', 'developer', 'admin'] as const).find((one) => one === body.default_role) === undefined ? {} : { default_role: body.default_role as IdentityProvider['default_role'] }),
      revision: before.revision + 1,
    };
    this.identityProviders = this.identityProviders.map((one) => (one.id === id ? made : one));
    this.audit(by, 'identity_provider.changed', `${by.me.display_name} changed the identity provider ${made.name}`);
    this.world.users.tellAll({ what: 'meta_changed', ...(requestId === undefined ? {} : { request_id: requestId }) });
    return made;
  }

  private deleteProvider(by: Person, id: string, requestId?: string): Answer {
    const found = this.identityProviders.find((one) => one.id === id);
    if (found === undefined) throw new Refusal(404, 'not_found', 'no such identity provider');
    this.identityProviders = this.identityProviders.filter((one) => one.id !== id);
    this.audit(by, 'identity_provider.removed', `${by.me.display_name} removed the identity provider ${found.name}`);
    this.world.users.tellAll({ what: 'meta_changed', ...(requestId === undefined ? {} : { request_id: requestId }) });
    return { status: 204 };
  }

  /** The test of a provider: the fake asks no issuer, and finds the one the provider names, signing with one key. */
  private testProvider(id: string): Answer {
    const found = this.identityProviders.find((one) => one.id === id);
    if (found === undefined) throw new Refusal(404, 'not_found', 'no such identity provider');
    return { status: 200, body: { ok: true, error: null, issuer_metadata: { issuer: found.issuer, signing_keys: 1 } } };
  }
}

// ---------------------------------------------------------------- small makers

function cap(key: string, label: string, risk: CapabilityDef['risk'], decision: CapabilityDef['default_decision'], scope: CapabilityDef['default_approver_scope'], description: string): CapabilityDef {
  return { key, label, description, risk, default_decision: decision, default_approver_scope: scope, driver_patch: { docker: {} }, builtin: true, revision: 1 };
}

function build(kind: string, version: string, isDefault: boolean, source: HarnessBuild['source'], url = '', sha = '', os: HarnessBuild['os'] = 'linux', arch: HarnessBuild['arch'] = 'x86_64'): HarnessBuild {
  return {
    kind,
    version,
    is_default: isDefault,
    source,
    os,
    arch,
    url: url === '' ? `https://downloads.example/${kind}/${version}/${kind}-${os}-${arch}.tar.gz` : url,
    sha256: sha === '' ? '0123456789abcdef'.repeat(4) : sha,
    // A release's builds are checked with the release; others wait for their check.
    status: source === 'release' ? 'checked' : 'untested',
  };
}

/** One page of a list that is read a page at a time: the cursor is the place in the list. */
function pageOf<Item>(all: readonly Item[], query: URLSearchParams): { items: Item[]; next_cursor: string | null } {
  const size = Math.min(PAGE_MAX, Math.max(1, Number(query.get('limit') ?? PAGE_DEFAULT)));
  const start = Number(query.get('cursor') ?? '0');
  const items = all.slice(start, start + size);
  return { items, next_cursor: start + size < all.length ? String(start + size) : null };
}
