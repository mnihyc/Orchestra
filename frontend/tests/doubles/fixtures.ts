import { executionDisplay } from './thread-display.ts';
/** What the fake API starts from: a system, its people, an org with its tree, and invitations, each in the shape the real API answers in. */
import type {
  Access,
  InvitationPreview,
  Me,
  Meta,
  NodeRef,
  Org,
  TreeNode,
  UserRef,
} from '../../src/generated/api-types.ts';

/** The fake API supplies one unmeasured model estimate, without classifying names. */
export const MODEL_ESTIMATE = { token_estimate: { ascii_prose_x100: 400, ascii_structured_x100: 350, other_x100: 100, image_tokens: 1000, measured: false } };

/** An identifier that is the same in every run: `kind` says what it names, `n` which one. */
export function id(kind: number, n: number): string {
  return `00000000-0000-4000-8${kind.toString(16).padStart(3, '0')}-${n.toString(16).padStart(12, '0')}`;
}

/** A scenario that explicitly needs a personal home must declare that fixture premise. */
export function personalHomeOf(person: Pick<Me, 'personal_org_id'>): string {
  if (person.personal_org_id === null) throw new Error('This fixture requires a personal organization.');
  return person.personal_org_id;
}

/** The kinds of resource, for the part of an identifier that says which one it is. */
export const KIND = { machine: 0x21, provider: 0x22, agent: 0x23, environment: 0x24, host: 0x25, template: 0x26, model: 0x27, thread: 0x28, runtime: 0x29, job: 0x2a } as const;

/**
 * What the team's threads run on, by identifier: the machine, the providers, the agent profiles and the
 * environments that every part of the fake API speaks of by the same name. A part that needs one of
 * them (a topic naming the environment its threads work in, a test opening the page of a provider)
 * takes the identifier from here and the rest from the world's own records.
 */
export const RUNS_ON = {
  laptop: id(KIND.machine, 1),
  openAiKey: id(KIND.provider, 1),
  codexSubscription: id(KIND.provider, 2),
  teamAnthropic: id(KIND.provider, 3),
  freshKey: id(KIND.provider, 4),
  claudeSubscription: id(KIND.provider, 5),
  codex: id(KIND.agent, 1),
  claudeCode: id(KIND.agent, 2),
  scripted: id(KIND.agent, 3),
  codexOnLaptop: id(KIND.agent, 4),
  workspace: id(KIND.environment, 1),
  docsCheckout: id(KIND.environment, 2),
  sandbox: id(KIND.environment, 3),
  buildHost: id(KIND.host, 2),
  defaultTemplate: id(KIND.template, 1),
} as const;

/** The model the team's threads usually run on: the name a request carries, the name it is shown under, and the effort it is usually asked for. */
export const USUAL_MODEL = { name: 'gpt-5.4', display: 'GPT-5.4', effort: 'high' } as const;

/** The moment the fixtures were "made": fixed, so that nothing depends on the day a test runs. */
export const MADE_AT = '2026-10-01T09:00:00.000Z';
/** Half an hour later: when the threads of a topic that is being worked on last did something. */
export const LATER_AT = '2026-10-01T09:30:00.000Z';

export function meta(overrides: Partial<Meta> = {}): Meta {
  return {
    name: 'Orch (double)',
    version: '0.1.0',
    build: 'double 1',
    needs_setup: false,
    registration_mode: 'invite',
    // The double's world lets anybody make a team org, as its scenarios were written; Orch ships `invited`.
    org_creation: 'anyone',
    may_create_org: true,
    mail: true,
    local_login: true,
    email_code_login: true,
    identity_providers: [],
    locales: ['en', 'zh-CN'],
    banner: null,
    push_public_key: null,
    link_endpoints: [],
    limits: { max_input_chars: 100_000, max_image_bytes: 20 * 1024 * 1024, max_upload_bytes: 64 * 1024 * 1024 },
    ...overrides,
  };
}

export function me(n: number, username: string, displayName: string, overrides: Partial<Me> = {}): Me {
  return {
    id: id(1, n),
    kind: 'human',
    username,
    display_name: displayName,
    avatar: { text: Array.from(displayName)[0]?.toUpperCase() ?? '', color: n % 12 },
    email: `${username}@example.org`,
    email_verified: true,
    email_pending: null,
    email_code: [],
    email_code_second_step: false,
    second_step_required: false,
    second_step_sources: [],
    has_password: true,
    totp_enabled: false,
    identities: [],
    is_system_admin: false,
    locale: 'en',
    timezone: 'UTC',
    timezone_mode: 'fixed',
    theme: 'system',
    density: 'comfortable',
    personal_org_id: id(2, n),
    created_at: MADE_AT,
    ...overrides,
  };
}

export function userRef(person: Me): UserRef {
  return {
    id: person.id,
    kind: person.kind,
    username: person.username,
    display_name: person.display_name,
    avatar: person.avatar,
  };
}

export const memberAccess: Access = {
  admin_only: [],
  participating: true,
  administers: false,
  owns: false,
  permissions: ['node.view'],
  role: 'developer',
  role_from: null,
  subscription: null,
  effective_subscription: 'off',
};

export function org(
  orgId: string,
  slug: string,
  name: string,
  owner: UserRef,
  overrides: Partial<Org> = {},
): Org {
  const self: NodeRef = { id: orgId, kind: 'org', name, redacted: false };
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
    color: null,
    readme: null,
    title_source: 'user',
    description_source: 'user',
    tags: [],
    tags_source: 'user',
    auto_updated_at: null,
    path: [self],
    owner,
    owners: 1,
    page: 'follows',
    access: memberAccess,
    archived: false,
    archived_at: null,
    archived_by: null,
    chat_id: id(9, Number.parseInt(orgId.slice(-6), 16)),
    closed: false,
    flat: false,
    member_count: 1,
    people_count: 1,
    org_count: 0,
    project_count: 0,
    changed_at: MADE_AT,
    revision: 1,
    created_at: MADE_AT,
    updated_at: MADE_AT,
    ...overrides,
  };
}

/** The ids of the team's tree, by what each is. */
export const ACME = {
  org: id(2, 100),
  research: id(3, 1),
  web: id(3, 2),
  fixLogin: id(3, 3),
  mainThread: id(3, 4),
  repro: id(3, 5),
  upgradeDeps: id(3, 6),
  depsThread: id(3, 7),
  migrateCi: id(3, 8),
  infra: id(3, 9),
  idleStop: id(3, 10),
  idleThread: id(3, 11),
  payroll: id(3, 12),
  ml: id(3, 13),
  evals: id(3, 14),
  baseline: id(3, 15),
  baselineThread: id(3, 16),
  /** A project of the org that has folders: Checkout, Payments inside that, a topic in each of them and a thread in the inner topic. */
  shop: id(3, 17),
  checkout: id(3, 18),
  payments: id(3, 19),
  cartRewrite: id(3, 20),
  cardFees: id(3, 21),
  feeTable: id(3, 22),
} as const;

/** Retained objects of the team's ordinary tree, for following a deletion without restoring it. */
export const TRACE = {
  project: id(27, 1), topic: id(27, 2), thread: id(27, 3), document: id(27, 4), purged: id(27, 5), child: id(27, 6), environment: id(27, 7), unknownEnvironment: id(27, 8),
} as const;

function node(
  nodeId: string,
  kind: TreeNode['kind'],
  name: string,
  parentId: string | null,
  seq: number,
  more: Partial<TreeNode> = {},
): TreeNode {
  return {
    id: nodeId,
    kind,
    name,
    address: null,
    parent_id: parentId,
    color: null,
    docs: 0,
    archived: false,
    deleted: false,
    restricted: false,
    participating: true,
    chat: { id: id(9, seq), seq: 0 },
    thread: null,
    topic: kind === 'topic' ? { status: 'open', tag_ids: [] } : null,
    tree_seq: seq,
    ...more,
  };
}

function thread(state: NonNullable<TreeNode['thread']>['state'], more: Partial<NonNullable<TreeNode['thread']>> = {}) {
  return {
    thread: {
      display: executionDisplay(state, { blocked_reason: more.blocked_reason ?? null }),
      state,
      attention: { uncommitted: null, unpushed: null, failed: false, as_of: null, stale: false, reason: null },
      blocked_reason: null,
      pending_approvals: 0,
      activity_seq: 1,
      last_activity_at: MADE_AT,
      ...more,
    },
  };
}

/** The tree of the team's org: orgs inside it, projects, topics open and done, threads in each state. */
export function acmeTree(): TreeNode[] {
  return [
    node(ACME.org, 'org', 'Acme', null, 1, { address: 'acme' }),
    node(ACME.research, 'org', 'Research', ACME.org, 2, { address: 'acme.research' }),
    node(ACME.web, 'project', 'Web', ACME.research, 3),
    node(ACME.fixLogin, 'topic', 'Fix login', ACME.web, 4),
    node(ACME.mainThread, 'thread', 'Main thread', ACME.fixLogin, 5, thread('running', { last_activity_at: LATER_AT })),
    node(ACME.repro, 'thread', 'Repro', ACME.fixLogin, 6, thread('waiting_approval', { pending_approvals: 1, last_activity_at: LATER_AT })),
    node(ACME.upgradeDeps, 'topic', 'Upgrade deps', ACME.web, 7),
    node(ACME.depsThread, 'thread', 'Main thread', ACME.upgradeDeps, 8, thread('idle')),
    node(ACME.migrateCi, 'topic', 'Migrate CI', ACME.web, 9, { topic: { status: 'done', tag_ids: [] } }),
    node(ACME.ml, 'org', 'ML', ACME.research, 10, { address: 'acme.research.ml' }),
    node(ACME.evals, 'project', 'Evals', ACME.ml, 11),
    node(ACME.baseline, 'topic', 'Baseline run', ACME.evals, 12),
    node(ACME.baselineThread, 'thread', 'Main thread', ACME.baseline, 13, thread('starting')),
    node(ACME.infra, 'project', 'Infra', ACME.org, 14),
    node(ACME.idleStop, 'topic', 'Idle-stop rework', ACME.infra, 15),
    node(
      ACME.idleThread,
      'thread',
      'Main thread',
      ACME.idleStop,
      16,
      thread('blocked', { blocked_reason: 'provider_exhausted' }),
    ),
    node(ACME.payroll, 'project', 'Payroll', ACME.org, 17, { restricted: true }),
    node(ACME.shop, 'project', 'Shop', ACME.org, 18),
    node(ACME.checkout, 'project', 'Checkout', ACME.shop, 19),
    node(ACME.cartRewrite, 'topic', 'Cart rewrite', ACME.checkout, 20),
    node(ACME.payments, 'project', 'Payments', ACME.checkout, 21),
    node(ACME.cardFees, 'topic', 'Card fees', ACME.payments, 22),
    node(ACME.feeTable, 'thread', 'Fee table', ACME.cardFees, 23, thread('idle')),
  ];
}

/** A person's own org: empty, as it is made with them. */
export function personalTree(orgId: string, name: string, address: string): TreeNode[] {
  return [node(orgId, 'org', name, null, 1, { address })];
}

export function invitation(inviter: UserRef, overrides: Partial<InvitationPreview> = {}): InvitationPreview {
  return {
    kind: 'org',
    inviter,
    org: { name: 'Acme', slug: 'acme' },
    role: 'developer',
    permissions: [],
    what: null,
    email_bound: false,
    expires_at: '2026-12-31T12:00:00.000Z',
    ...overrides,
  };
}
