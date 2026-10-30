/** The one registry of the web application's acts: for every request that changes something, the data it changes, and the one way a change reaches every view that shows it.
 *
 * Every screen reads through the query cache, under keys of the form `[kind of data, which one, how it was
 * asked]`. An act that succeeds is told here by the API client (`client.onAct`), before its caller hears of
 * it; its native answer replaces the displayed row, its loaded lists and references before the act resolves. So a dialog that closes when its
 * act succeeds leaves the views beneath and beside it already showing the change, and no screen refreshes
 * anything by hand. What other people, and the person's other tabs, change arrives by the live connection,
 * whose handlers name the same changes through the same function (`applyChanges`): a row of an org's tree, the
 * person's own news, the feed of a resource.
 *
 * The catalogue retains dependency hints for old endpoints. answerChanges converts them to native row,
 * value or collection patches. If the answer genuinely omits a resulting projection, only an exact
 * visible query is repaired; no act or reconnect invalidates a whole kind. Authority changes discard
 * old access-dependent content and repair exact visible objects, retaining personal preferences.
 *
 * A request with no entry here is a fault: it is reported where it happens, and a unit test reads every
 * request of `src/` that changes something and fails on one that has none (`tests/unit/acts.test.ts`). A
 * request that changes nothing a screen reads (a preview, a ticket, a part of an upload) has an entry that
 * says so.
 */
import type { Query, QueryClient, QueryKey } from '@tanstack/react-query';

import { type Act, client } from '@/api/client';
import { ENROLLMENT_PROGRESS, enrollmentChanges, enrollmentProgressKey, type EnrollmentChange } from '@/api/enrollment';
import { answerChanges } from '@/api/actAnswers';
import { applyAdminUser, repairAdminOrgs, repairMeta, type AdminOrgRestoreChange, type AdminUserChange, type MetaChange } from '@/api/adminState';
import { AGENT_REACH, agentReachProjections, type AgentReachChange } from '@/api/agentReach';
import { applyCollection, applyRow, applyThread, missingInventoryProjection, record, type CollectionChange, type RowChange, type ThreadChange, type ValueChange } from '@/api/cache';
import { applyDocumentTreeChange, repairDocumentDirectories, type DocumentTreeChange, type DocumentDirectoryChange } from '@/api/documentTreeChanges';
import { repairResource, type ResourceRepair } from '@/api/resourceRepair';
import { queryClient } from '@/api/query';
import { capturePeopleHint, readAuthority, repairPeople } from '@/api/peopleChanges';
import type { BindingOptions, TreeNode } from '@/generated/api-types';
import { queryFeed } from '@/realtime/queryFeeds';
import { queryAuthorityAllows } from '@/api/queryAuthority';

// ---------------------------------------------------------------- what a change is

/** Data the query cache keeps: the queries whose key begins with `key`, narrowed to those that name `id` anywhere in it. */
export interface Stale {
  key: readonly string[];
  /** Only the queries that name this identifier. */
  id?: string;
  /** A query argument at a fixed position, for views such as the finder's empty-query recents. */
  at?: { index: number; value: string };
  /** Only the query whose key is exactly `key`: what lies under it (a file's bytes) did not change. */
  exact?: true;
  /** The thing is gone: what was read of it is not asked for again, and is dropped once nothing shows it. */
  gone?: true;
}

/** Data a live feed keeps: the trees of the orgs held, a thread's conversation, a document's blocks, the two counters. */
export interface Followed {
  feed: 'trees' | 'thread' | 'document' | 'counters';
  id?: string;
}

export interface SessionAuthorityChanged { session: true; mode?: unknown; asAdmin?: boolean; person?: unknown }
export interface StateChange { state: { key: string; data: unknown; child?: string } }
/** An answer genuinely omitted its view: choose one exact currently shown projection, never stale a kind. */
export interface MissingProjection {
  missing: readonly Stale[];
  /** A bodyless act names a row, while its complete list is keyed by the owning place. */
  containingRow?: string;
  /** Service accounts are identified by their nested user reference. */
  containingAccount?: string;
  cancelPending?: true;
}
export type Change = AdminOrgRestoreChange | AdminUserChange | MetaChange | Stale | Followed | SessionAuthorityChanged | RowChange | ThreadChange | ValueChange | CollectionChange | StateChange | ResourceRepair | MissingProjection | AgentReachChange | EnrollmentChange | DocumentTreeChange | DocumentDirectoryChange | { people: true; authority?: true; cut?: number } | { approvalViews: true } | { relatedViews: true } | { capViews: { id: string; value?: Record<string, unknown> } } | { rateViews: { nodeId: string | null } };

/** The value of a parameter of the request's path, by its name in the template: `p('id')`. */
export type PathParam = (name: string) => string;

/** What an act changed, from the parameters of its path. */
export type Entry = (p: PathParam) => readonly Change[];

const one = (id: string, ...key: string[]): Stale => ({ key, id });
const every = (...key: string[]): Stale => ({ key });
const only = (...key: string[]): Stale => ({ key, exact: true });
const gone = (id: string, ...key: string[]): Stale => ({ key, id, gone: true });
const kinds = (...heads: string[]): Stale[] => heads.map((head) => every(head));
const ones = (id: string, ...heads: string[]): Stale[] => heads.map((head) => one(id, head));
const TREES: Followed = { feed: 'trees' };
const COUNTERS: Followed = { feed: 'counters' };
const conversation = (id: string): Followed => ({ feed: 'thread', id });
const blocks = (id: string): Followed => ({ feed: 'document', id });
/** A request that changes nothing a screen reads. */
const NOTHING: readonly Change[] = [];
/** The views of one thing that was removed: what was read of it alone is gone; the lists it was a row of are asked again. */
const ended = (changes: readonly Change[], id: string): Change[] => changes.map((change) => ('key' in change && change.id === id ? { ...change, gone: true } : change));

// ---------------------------------------------------------------- what is read of each kind of thing

/** Every list a place is a row of, and what counts the places in it. */
const placeLists = (): Change[] => [
  ...kinds('node', 'org', 'orgs', 'children', 'topics', 'activity', 'deleted', 'everything', 'names', 'recents', 'favorites'),
  ...kinds('shared-with-me', 'tree-places', 'places', 'finder-tag-projects', 'first-steps', 'trace', 'outside-port-openings', 'user-shares'),
  TREES,
];

/** A place was made, renamed, archived or brought back: it, the lists it is a row of, and the trees. */
const place = (id: string): Change[] => [...ones(id, 'node', 'children', 'topics', 'activity', 'tags', 'documents'), ...placeLists()];

/** Who may do what with what lies in a place changed: its page, its defaults or its roles, which reach everything inside it. */
const placeAccess = (): Change[] => [
  { relatedViews: true },
  ...kinds('page', 'page-people', 'page-effective', 'page-history', 'defaults', 'roles', 'preview', 'agent-shares'),
  ...kinds('thread-shared', 'thread-shared-tree', 'environment-reach', AGENT_REACH, 'access-requests', 'holder-search', 'node', 'members'),
  ...kinds('holdings', 'first-steps', 'usage', 'start-options', 'shared-with-me', 'outside-port-openings'),
  ...kinds('thread-instructions', 'model-inputs', 'model-input-block', 'user-shares'),
  TREES,
];

/** A place moved, or one with what is in it was deleted or brought back: its rows, and what follows from where it lies. */
const placeMoved = (): Change[] => [...placeLists(), ...placeAccess(), ...kinds('settings', 'secrets', 'documents', 'environments', 'usage')];

/** What the page of each kind of thing that is no place is read by, beside the page itself. */
const THING_VIEWS: Readonly<Record<string, readonly string[]>> = {
  documents: ['document', 'documents', 'readme'],
  chats: ['chat', 'chats', 'chat-states'],
  providers: ['provider', 'providers', 'provider-setup', 'provider-threads', 'start-options', 'usage'],
  agents: ['agent', 'agents', 'start-options'],
  environments: ['environment', 'environments', 'environment-reach', AGENT_REACH, 'environment-ports', 'start-options', 'mounts', 'machine-environment-data'],
  hosts: ['host', 'host-settings', 'hosts', 'host-runtimes', 'mounts', 'machine-environment-data'],
  templates: ['template', 'templates'],
  'mcp-servers': ['mcp-server', 'mcp-servers'],
  mounts: ['mount', 'mounts', 'mount-uses'],
  skills: ['skill', 'skills', 'skill-versions', 'skill-draft'],
};

/** Who may do what with one thing changed (`object` is how the API addresses its kind); a place's change reaches what is in it. */
const access = (object: string, id: string): Change[] => {
  if (object === 'nodes') return [...placeAccess(), one(id, 'node')];
  const views = THING_VIEWS[object] ?? [];
  return [
    { relatedViews: true },
    ...ones(id, 'page-people', 'page-effective', 'page-history', 'agent-shares'),
    ...(object === 'documents' ? [blocks(id)] : []),
    // Another page may say what this one gives: a thread's page says whether the people on it reach its environment.
    ...kinds('page', 'preview', 'holder-search', 'access-requests', 'shared-with-me', 'user-shares'),
    ...views.map((head, index) => (index === 0 ? one(id, head) : every(head))),
  ];
};

/** Settings resolve down the tree: what a place sets is what every place, thread and resource inside it reads. */
const settings = (): Change[] =>
  kinds('settings', 'secrets', 'thread-parameters', 'thread-limits', 'start-options', 'usage', 'usage-caps', 'first-steps', 'page', 'defaults', 'preview', 'provider-settings', 'environment-settings', 'host-settings', AGENT_REACH, 'environment-ports', 'notifiers', 'channels');

const relatedSettings = (key: string): Change[] => key.startsWith('related.') || key.startsWith('search.semantic.') ? [{ relatedViews: true }] : [];

/** Who is in an org changed: its members, what they hold, and every list that names people of it. */
export const people = (authority = false): Change[] => [{ people: true, ...(authority ? { authority: true } : {}) }];

/** A group, which is a holder on pages and a set of people behind it. */
const groups = (): Change[] => kinds('groups', 'org-groups', 'group-members', 'group-holders', 'holder-search', 'page', 'page-people', 'page-effective', 'roles', 'defaults', 'usage', 'preview');

/** A tag, which topics carry. */
const tags = (): Change[] => [...kinds('tags', 'topics', 'node', 'finder-tag-projects'), TREES];

/** A thread's own views: its conversation by its feed, and what is read of it beside. */
const thread = (id: string): Change[] => [
  conversation(id),
  every('environment-reach'),
  every(AGENT_REACH),
  ...ones(id, 'node', 'thread', 'thread-parameters', 'thread-limits', 'thread-outline', 'thread-catchup', 'thread-sides', 'thread-instructions', 'model-inputs', 'thread-items'),
  ...ones(id, 'thread-context', 'thread-turn-usage', 'turn-cost'),
];

/** A fresh thread row carries settings changes even before its first turn, so its independent reads are asked again. */
export function threadRowChanges(id: string): Change[] {
  return ones(id, 'node', 'thread', 'thread-parameters', 'settings');
}

/** Threads were made, moved or changed where they run: the threads, their places and the trees. */
const threads = (): Change[] => [...kinds('node', 'topics', 'thread-sides', 'provider-threads', 'environments', 'environment', 'environment-reach', AGENT_REACH, 'start-options', 'thread-parameters', 'job', 'fork-batch'), TREES];

/** A document's own views: its blocks by its feed, and what is read of it beside. */
const doc = (id: string): Change[] => [blocks(id), ...ones(id, 'document', 'document-versions', 'document-changes', 'document-readers'), ...kinds('documents', 'readme')];

/** One environment, as its page, the inspector beside a thread and the lists show it. */
const environment = (id: string): Change[] => [
  every('machine-environment-data'),
  every('mounts'),
  ...ones(id, 'environment', 'environment-settings', 'environment-reach', AGENT_REACH, 'environment-ports', 'terminals', 'processes', 'resources'),
  ...kinds('environments', 'start-options', 'host', 'host-runtimes', 'hosts', 'trace-deleted', 'deleted-inspection'),
];

/** The files of an environment's workspace, by every view of them. The bytes of a file are keyed by its hash and never change. */
const files = (id: string): Change[] => [...ones(id, 'workspace-stat', 'workspace-changes', 'workspace-git', 'workspace-git-diff', 'workspace-head', 'workspace-search', 'workspace-archive', 'archive-entry')];

/** One provider, with what its pages and every chooser of a provider read. */
const provider = (id: string): Change[] => [
  ...ones(id, 'provider', 'provider-settings', 'provider-models', 'provider-suggestions', 'provider-threads', 'provider-caps', 'provider-login', 'settings'),
  ...kinds('providers', 'provider-setup', 'start-options', 'usage', 'thread-parameters', AGENT_REACH),
];

const agent = (id: string): Change[] => [one(id, 'agent'), ...kinds('agents', 'start-options', 'harness-kinds')];
const host = (id: string): Change[] => [...ones(id, 'host', 'host-settings', 'host-runtimes', 'host-ports'), ...kinds('hosts', 'environments', 'admin', 'mounts')];
const template = (id: string): Change[] => [one(id, 'template'), ...kinds('templates', 'start-options', 'provider-setup')];
const machine = (id: string): Change[] => [...ones(id, 'machine', 'machine-policy'), ...kinds('machines', 'providers', 'agents', 'environments', 'start-options', 'provider-setup', 'mounts', 'machine-environment-data')];
const mcp = (id: string): Change[] => [one(id, 'mcp-server'), every('mcp-servers')];
const mount = (id: string): Change[] => [...ones(id, 'mount', 'mount-uses'), ...kinds('mounts', AGENT_REACH)];
const skill = (id: string): Change[] => [...ones(id, 'skill', 'skill-versions', 'skill-draft'), ...kinds('skills', 'skill-version', 'skill-version-file', 'skill-draft-file')];

/** A chat's own data beside its messages, which its feed keeps: the chat, the lists of chats and what the person keeps of each. */
const chat = (id: string): Change[] => [only('chat', id), ...kinds('chats', 'chat-states')];

/** What the person was told and what waits for them. */
const inbox = (): Change[] => [...kinds('inbox', 'away', 'notification-prefs'), COUNTERS];

/** Requests that wait for a decision: every list of them and the count. */
const approvals = (): Change[] => [...kinds('approvals', 'approval', 'access-requests', 'access-request', 'port-requests'), COUNTERS];

/** How the person is told outside Orch. */
const routes = (): Change[] => kinds('notification-channels', 'notification-prefs', 'away', 'notifiers', 'channels', 'first-steps');

/** Usage: figures, caps, limits and rates, which each screen of usage reads under one head. */
const usage = (): Change[] => kinds('usage', 'usage-caps', 'provider-caps', 'thread-limits');

/** What a job that changes where work runs reaches: environments, providers, hosts, the threads bound to them. */
const placement = (): Change[] => [
  ...kinds('environment', 'environments', 'environment-reach', AGENT_REACH, 'host', 'hosts', 'host-runtimes', 'provider', 'providers', 'job', 'fork-batch'),
  ...kinds('thread-parameters', 'start-options', 'node', 'thread-sides', 'change-plan', 'change-env-plan', 'change-batch-plan', 'recycle-dry-run'),
  TREES,
];

/** Who the person is, as every screen reads them. */
const me = (): Change[] => kinds('me', 'meta', 'profile', 'profile-activity', 'sessions', 'tokens', 'first-steps');

// ---------------------------------------------------------------- the registry

/**
 * Every request of the web application that changes something, by its method and its path as the API writes it,
 * with what it changes. Grouped by what it acts on.
 */
export const ACTS: Readonly<Record<string, Entry>> = {
  'PUT /me/admin-mode': () => [{ session: true }],
  'POST /me/admin-openings': () => kinds('trace'),
  // -- signing in and out: the session is the page's whole state, and the session's own events clear or set it
  'POST /setup': () => [...me(), every('orgs')],
  'POST /auth/login': () => NOTHING,
  'POST /auth/email-code': () => NOTHING,
  'POST /auth/register': () => NOTHING,
  'POST /auth/recovery': () => NOTHING,
  'POST /auth/recovery/confirm': () => kinds('recovery-token'),
  'POST /auth/verify-email': () => me(),
  'POST /auth/verify-email/resend': () => NOTHING,

  // -- the person themself
  'PATCH /me': () => [...me(), ...kinds('members', 'people', 'people-search')],
  'PATCH /me/profile': () => me(),
  'PUT /me/profile/pins': () => me(),
  'POST /me/password': () => kinds('sessions', 'me'),
  'POST /me/totp/setup': () => NOTHING,
  'POST /me/totp/enable': () => kinds('me', 'recovery-codes'),
  'POST /me/recovery-codes': () => kinds('recovery-codes'),
  'POST /me/totp/disable': () => me(),
  'POST /me/email-code/send': () => NOTHING,
  'POST /me/email-code/enable': () => me(),
  'POST /me/email-code/disable': () => me(),
  'POST /me/second-step/verify': () => kinds('me'),
  'POST /me/identities/{key}/link': () => me(),
  'DELETE /me/identities/{key}': () => me(),
  'DELETE /me/sessions/{id}': () => kinds('sessions'),
  'DELETE /me/sessions': () => kinds('sessions'),
  'POST /me/tokens': () => kinds('tokens', 'org-tokens'),
  'DELETE /me/tokens/{id}': () => kinds('tokens', 'org-tokens'),
  'PUT /me/favorites/{node_id}': () => kinds('favorites', 'names'),
  'DELETE /me/favorites/{node_id}': () => kinds('favorites', 'names'),
  'PUT /me/favorites/{kind}/{id}': () => kinds('favorites', 'names'),
  'DELETE /me/favorites/{kind}/{id}': () => kinds('favorites', 'names'),
  'POST /me/visits': () => NOTHING,
  'POST /me/recents': () => NOTHING,
  // What the interface keeps is shown as it is set; reading it back while more is being sent could bring back an older
  // value. Other devices hear of it by the person's news (`state`).
  'PUT /me/state/{key}': () => NOTHING,
  'PUT /me/state/help': () => kinds('first-steps'),
  'PUT /me/first-steps': () => kinds('first-steps', 'me-state'),
  'PUT /me/state/project_views/{project_id}': () => NOTHING,

  // -- notifications and the ways a person is told
  'POST /notifications/read': () => inbox(),
  'PUT /me/notification-prefs': () => [...routes(), ...inbox()],
  // answerChanges installs the returned exception page and ordinary Access; no follow-up read.
  'PUT /me/notification-exceptions/{id}': () => [],
  'PUT /me/notification-routes/{channel_id}': () => routes(),
  'DELETE /me/notification-routes/{channel_id}': () => routes(),
  'DELETE /me/notification-routes/by-id/{route_id}': () => routes(),
  'POST /me/notification-routes/{channel_id}/link': () => routes(),
  'POST /me/notification-channels': () => routes(),
  'POST /me/push-subscriptions': () => routes(),
  'DELETE /me/push-subscriptions': () => routes(),
  'DELETE /me/push-subscriptions/{id}': () => routes(),
  'POST /notification-channels/{id}/test': () => routes(),
  'PATCH /notification-channels/{id}': () => routes(),
  'DELETE /notification-channels/{id}': () => routes(),
  'POST /orgs/{id}/notification-channels': () => routes(),
  'POST /admin/notification-channels': () => routes(),
  'POST /nodes/{id}/destinations': (p) => [one(p('id'), 'destinations'), ...routes()],
  'PATCH /destinations/{id}': () => [every('destinations'), ...routes()],
  'DELETE /destinations/{id}': () => [every('destinations'), ...routes()],
  'POST /destinations/{id}/test': () => [every('destinations')],

  // -- places: orgs at every level, projects, folders, topics
  'POST /orgs': () => [...placeLists(), ...me(), every('admin')],
  'POST /orgs/{id}/orgs': (p) => [...place(p('id')), every('admin')],
  'POST /orgs/{id}/projects': (p) => place(p('id')),
  'POST /projects/{id}/projects': (p) => place(p('id')),
  'POST /projects/{id}/topics': (p) => place(p('id')),
  'PATCH /nodes/{id}': (p) => [...place(p('id')), every('page'), every('readme'), every('admin'), conversation(p('id'))],
  'PATCH /topics/{id}': (p) => [...place(p('id')), ...tags()],
  'POST /nodes/{id}/auto': (p) => [...place(p('id')), ...thread(p('id'))],
  'POST /nodes/{id}/archive': (p) => [...place(p('id')), ...kinds('documents', 'environments')],
  'POST /resources/library': () => NOTHING,
  'POST /resources/library/people': () => NOTHING,
  'POST /nodes/{id}/archive/preview': () => NOTHING,
  'POST /nodes/{id}/unarchive/preview': () => NOTHING,
  'POST /nodes/{id}/unarchive': (p) => [...place(p('id')), ...kinds('documents', 'environments')],
  'POST /nodes/{id}/delete/preview': () => NOTHING,
  // What lies inside the deleted place is gone too, and only the tree knows what that is: every place is asked again
  // once the tree's deleted rows have said which are gone (`treeChanges`), so a screen of one of them is not asked again.
  'DELETE /nodes/{id}': (p) => [
    ...ended(ones(p('id'), 'node', 'children', 'topics', 'activity', 'tags', 'documents', 'settings', 'roles', 'page', 'defaults'), p('id')),
    ...placeMoved().filter((change) => !('key' in change && change.key[0] === 'node' && change.id === undefined)),
    every('admin'), every('thread-sides'),
  ],
  'POST /nodes/{id}/restore': (p) => [...place(p('id')), ...placeMoved(), ...kinds('trace-deleted', 'deleted-inspection', 'admin')],
  'POST /projects/{id}/move/preview': () => NOTHING,
  'POST /topics/{id}/move/preview': () => NOTHING,
  'POST /orgs/{id}/move/preview': () => NOTHING,
  'POST /projects/{id}/move': (p) => [...place(p('id')), ...placeMoved()],
  'POST /topics/{id}/move': (p) => [...place(p('id')), ...placeMoved()],
  'POST /orgs/{id}/move': (p) => [...place(p('id')), ...placeMoved(), ...people()],
  'PUT /nodes/{id}/subscription': (p) => [one(p('id'), 'node'), ...kinds('away')],
  'DELETE /nodes/{id}/subscription': (p) => [one(p('id'), 'node'), ...kinds('away')],
  'POST /nodes/{id}/tags': (p) => [one(p('id'), 'tags'), ...tags()],
  'PUT /nodes/{id}/tags': (p) => [...place(p('id')), ...tags(), ...kinds('everything')],
  'POST /nodes/{id}/announcements': (p) => [...place(p('id')), ...kinds('announcements')],
  'PATCH /tags/{id}': () => tags(),
  'DELETE /tags/{id}': () => tags(),
  'PUT /topics/{id}/default-environment': (p) => [...place(p('id')), ...kinds('start-options', 'environments')],
  'POST /nodes/{id}/transfer': (p) => [...place(p('id')), ...placeAccess(), every('admin')],

  // -- access: pages, defaults, roles, requests, agents' shares
  'PATCH /{object}/{id}/permissions': (p) => access(p('object'), p('id')),
  'POST /{object}/{id}/permissions/preview': () => NOTHING,
  'POST /{object}/{id}/share/preview': () => NOTHING,
  'PATCH /nodes/{id}/defaults': (p) => [...placeAccess(), one(p('id'), 'defaults')],
  'POST /nodes/{id}/defaults/preview': () => NOTHING,
  'PATCH /admin/defaults': () => placeAccess(),
  'POST /admin/defaults/preview': () => NOTHING,
  'POST /nodes/{id}/roles': () => placeAccess(),
  'PATCH /roles/{id}': () => placeAccess(),
  'DELETE /roles/{id}': () => placeAccess(),
  'POST /access-requests': () => approvals(),
  'POST /chats/{id}/join-requests': () => approvals(),
  'POST /approvals/{id}/cancel': () => approvals(),
  'POST /approvals/{id}/decision': () => [...approvals(), ...placeAccess(), ...kinds('thread-parameters', 'environment', 'environment-ports', 'port-requests', 'outside-port-openings')],
  'POST /share/agents': () => kinds('agent-shares', 'thread-shared', 'thread-shared-tree', 'page', 'page-history'),
  'PATCH /share/agents/{share_id}': () => kinds('agent-shares', 'thread-shared', 'thread-shared-tree', 'page', 'page-history'),
  'DELETE /share/agents/{share_id}': () => kinds('agent-shares', 'thread-shared', 'thread-shared-tree', 'page', 'page-history'),

  // -- members, invitations, groups, service accounts, outsiders
  'POST /invitations': () => kinds('invitations'),
  'DELETE /invitations/{id}': () => kinds('invitations'),
  'POST /invitations/accept': () => [...placeLists(), ...people(), ...me()],
  'DELETE /orgs/{id}/members/{user_id}': () => [...people(), ...placeLists(), ...kinds('providers', 'hosts', 'environments', 'agents')],
  'POST /orgs/{id}/members/{user_id}/suspend': () => people(),
  'POST /orgs/{id}/members/{user_id}/resume': () => people(),
  'POST /orgs/{id}/members/{user_id}/move/preview': () => NOTHING,
  'POST /orgs/{id}/members/{user_id}/move': () => [...people(), ...placeAccess()],
  'POST /orgs/{id}/outsiders/{user_id}/remove/preview': () => NOTHING,
  'POST /orgs/{id}/outsiders/{user_id}/remove': () => [...people(), ...placeAccess()],
  'POST /orgs/{id}/groups': () => groups(),
  'PATCH /groups/{id}': () => groups(),
  'DELETE /groups/{id}': (p) => [...groups(), gone(p('id'), 'group-members'), gone(p('id'), 'group-holders')],
  'PUT /groups/{id}/holders': () => groups(),
  'DELETE /groups/{id}/holders': () => groups(),
  'PUT /groups/{id}/members/{user_id}': () => groups(),
  'DELETE /groups/{id}/members/{user_id}': () => groups(),
  'POST /orgs/{id}/service-accounts': () => [...kinds('service-accounts'), ...people()],
  'DELETE /service-accounts/{id}': (p) => [...kinds('service-accounts'), gone(p('id'), 'service-tokens'), ...people()],
  'POST /service-accounts/{id}/tokens': (p) => [one(p('id'), 'service-tokens'), ...kinds('service-accounts', 'org-tokens')],
  'DELETE /service-accounts/{id}/tokens/{token_id}': (p) => [one(p('id'), 'service-tokens'), ...kinds('service-accounts', 'org-tokens')],
  'POST /orgs/{id}/tokens/{token_id}/refuse': (p) => [one(p('id'), 'org-tokens'), ...kinds('tokens', 'service-tokens')],

  // -- settings and secrets, at a place, of a resource, of the system
  'PUT /nodes/{id}/settings/{key}': (p) => [...settings(), ...relatedSettings(p('key')), ...thread(p('id')), one(p('id'), 'node')],
  'DELETE /nodes/{id}/settings/{key}': (p) => [...settings(), ...relatedSettings(p('key')), ...thread(p('id')), one(p('id'), 'node')],
  'PUT /nodes/{id}/secrets/{name}': () => kinds('secrets', 'settings'),
  'DELETE /nodes/{id}/secrets/{name}': () => kinds('secrets', 'settings'),
  'PUT /environments/{id}/secrets/{name}': (p) => [one(p('id'), 'secrets'), ...kinds('settings')],
  'DELETE /environments/{id}/secrets/{name}': (p) => [one(p('id'), 'secrets'), ...kinds('settings')],
  'PUT /admin/settings/{key}': (p) => [...settings(), ...relatedSettings(p('key')), ...kinds('meta', 'admin'), ...(p('key').startsWith('hosts.') ? kinds('host', 'hosts') : [])],
  'DELETE /admin/settings/{key}': (p) => [...settings(), ...relatedSettings(p('key')), ...kinds('meta', 'admin'), ...(p('key').startsWith('hosts.') ? kinds('host', 'hosts') : [])],

  // -- threads
  'POST /topics/{id}/threads': (p) => [...place(p('id')), ...threads()],
  'PUT /threads/{id}/environment': (p) => [...thread(p('id')), ...threads()],
  'PUT /threads/{id}/binding': (p) => [...thread(p('id')), ...threads()],
  // A session taken in is a thread of the topic, and the harness's list of sessions now names it.
  'POST /topics/{id}/threads/adopt': (p) => [...place(p('id')), ...threads(), ...kinds('harness-sessions')],
  'POST /topics/{id}/threads/adopt/preview': () => NOTHING,
  // The draft is the box's own: it was emptied as the message went, and the person may be writing the next one.
  'POST /threads/{id}/inputs': (p) => [conversation(p('id')), ...ones(p('id'), 'thread-limits', 'thread-catchup'), ...usage()],
  'PATCH /threads/{id}/inputs/{input_id}': (p) => [conversation(p('id'))],
  'DELETE /threads/{id}/inputs/{input_id}': (p) => [conversation(p('id')), one(p('id'), 'thread-limits')],
  // A 202 admits work. Its caller selects its own read window; native scoped feeds publish the outcome.
  // Refreshing the default window here would read an unrelated active pane or race that selection.
  'POST /threads/{id}/inputs/{input_id}/versions': () => NOTHING,
  'POST /threads/{id}/plans/{item_id}/decision': () => NOTHING,
  // The captured thread feed owns goal state; a default-window refresh could target another read.
  'POST /threads/{id}/goal': () => NOTHING,
  'PATCH /threads/{id}/goal': () => NOTHING,
  'DELETE /threads/{id}/goal': () => NOTHING,
  'POST /threads/{id}/continuations/{continuation_id}/activate': () => NOTHING,
  'POST /threads/{id}/inputs/{input_id}/retry': (p) => [conversation(p('id'))],
  'POST /threads/{id}/inputs/{input_id}/bypass': (p) => [conversation(p('id')), one(p('id'), 'thread-limits'), ...usage()],
  'POST /threads/{id}/inputs/{input_id}/run': (p) => [conversation(p('id')), ...usage()],
  'POST /threads/{id}/interrupt': (p) => thread(p('id')),
  'POST /threads/{id}/queue/resume': (p) => [conversation(p('id'))],
  'POST /threads/{id}/turns/{turn_id}/retry': (p) => [conversation(p('id'))],
  'POST /threads/{id}/calls/recovery': (p) => [conversation(p('id')), one(p('id'), 'call-recovery')],
  // A read position moves as the person reads: the Inbox and its counters hear of it by the person's news
  // (`notifications_read`), and asking at every step would only slow the reading.
  'POST /threads/{id}/read': () => NOTHING,
  // A draft is saved while it is written: reading it back could put older words under the person's hands.
  'PUT /threads/{id}/draft': () => NOTHING,
  'DELETE /threads/{id}/draft': () => NOTHING,
  'PUT /threads/{id}/items/{item_id}/pin': (p) => [conversation(p('id')), one(p('id'), 'thread-outline')],
  'DELETE /threads/{id}/items/{item_id}/pin': (p) => [conversation(p('id')), one(p('id'), 'thread-outline')],
  'PUT /threads/{id}/links': (p) => [conversation(p('id')), ...ones(p('id'), 'thread', 'workspace-git')],
  'DELETE /threads/{id}/approval-rules/{rule_id}': (p) => [one(p('id'), 'thread')],
  'POST /threads/{id}/background/end': (p) => [conversation(p('id'))],
  'POST /threads/{id}/attachments': () => NOTHING,
  'POST /threads/{id}/agent-terminal/ticket': () => NOTHING,
  'POST /threads/{id}/move/preview': () => NOTHING,
  'POST /threads/{id}/move': (p) => [...thread(p('id')), ...placeMoved(), ...threads()],
  'POST /threads/{id}/side': (p) => [...thread(p('id')), ...threads()],
  'POST /threads/{id}/adopt/retry-history': (p) => [...thread(p('id')), one(p('id'), 'adopt-history')],
  'POST /threads/{id}/forks/preflight': () => NOTHING,
  'POST /threads/{id}/forks': (p) => [...thread(p('id')), ...threads(), ...placement()],
  'POST /threads/{id}/forks/now': (p) => [...thread(p('id')), ...threads()],
  'POST /forks/preflight': () => NOTHING,
  'POST /forks': () => [...threads(), ...placement()],
  'POST /forks/{batch_id}/cancel': (p) => [one(p('batch_id'), 'fork-batch'), ...threads(), ...placement()],
  'POST /jobs/{id}/cancel': (p) => [one(p('id'), 'job'), ...threads(), ...placement(), every('exports')],
  'POST /storage/cleanup/preflight': () => NOTHING,
  'POST /resources/impact': () => NOTHING,
  'POST /storage/cleanup': () => [every('storage')],

  // -- chats among people
  'POST /chats/{id}/messages': (p) => chat(p('id')),
  'PATCH /chats/{id}/messages/{message_id}': (p) => chat(p('id')),
  'DELETE /chats/{id}/messages/{message_id}': (p) => [...chat(p('id')), every('chat', p('id'), 'pinned')],
  'PUT /chats/{id}/messages/{message_id}/reactions/{emoji}': () => NOTHING,
  'DELETE /chats/{id}/messages/{message_id}/reactions/{emoji}': () => NOTHING,
  'PUT /chats/{id}/messages/{message_id}/pin': (p) => [every('chat', p('id'), 'pinned')],
  'DELETE /chats/{id}/messages/{message_id}/pin': (p) => [every('chat', p('id'), 'pinned')],
  'POST /chats/{id}/read': () => NOTHING,
  'POST /orgs/{id}/chats/read': () => [...kinds('chats', 'chat-states'), ...inbox()],
  'PUT /chats/{id}/state': (p) => chat(p('id')),
  'PUT /chats/{id}/draft': () => NOTHING,
  'DELETE /chats/{id}/draft': () => NOTHING,
  'POST /orgs/{id}/chats': () => kinds('chats', 'chat-states'),
  'PATCH /chats/{id}': (p) => chat(p('id')),
  'PUT /chats/{id}/members/{user_id}': (p) => [...chat(p('id')), one(p('id'), 'chat-mentions')],
  'DELETE /chats/{id}/members/{user_id}': (p) => [...chat(p('id')), one(p('id'), 'chat-mentions')],
  'PUT /chats/{id}/agents/{thread_id}/posting': (p) => [...chat(p('id')), ...thread(p('thread_id'))],
  'POST /chats/{id}/mentions/preview': () => NOTHING,
  'POST /chats/{id}/attachments': () => NOTHING,
  'POST /forward': () => [...kinds('chats', 'chat-states', 'thread-item')],

  // -- documents
  'POST /document-tree/operations': () => kinds('document-directory', 'document-tree-entry'),
  'POST /document-tree/delete/preview': () => NOTHING,
  'POST /document-containers/{id}/documents': () => kinds('document-directory', 'document'),
  'POST /nodes/{id}/documents': (p) => [...ones(p('id'), 'documents', 'node'), ...kinds('readme', 'exports'), TREES],
  'POST /nodes/{id}/documents/copy': (p) => [...ones(p('id'), 'documents', 'node'), TREES],
  'POST /nodes/{id}/documents/import': (p) => [...ones(p('id'), 'documents', 'node'), TREES],
  'PATCH /documents/{id}': (p) => [...doc(p('id')), ...kinds('names', 'recents', 'node', 'user-shares'), TREES],
  'PUT /documents/{id}/links': (p) => [one(p('id'), 'document-links')],
  'DELETE /documents/{id}': (p) => [gone(p('id'), 'document'), gone(p('id'), 'document-versions'), ...kinds('documents', 'readme', 'node', 'names', 'trace-deleted', 'trace', 'user-shares'), TREES],
  'POST /documents/{id}/restore': (p) => [...doc(p('id')), ...kinds('node', 'trace-deleted', 'deleted-inspection', 'trace', 'user-shares'), TREES],
  'POST /documents/{id}/move/preview': () => NOTHING,
  'POST /documents/{id}/move': (p) => [...doc(p('id')), ...access('documents', p('id')), ...kinds('node'), TREES],
  // A batch of what is written is answered in the document's own feed, which keeps its blocks; nothing else reads them
  // while it is written, and a batch goes every few hundred milliseconds.
  'POST /documents/{id}/ops': () => NOTHING,
  // Reading a paste returns draft blocks and uploads inline assets; saving the blocks is a separate document act.
  'POST /documents/convert': () => NOTHING,
  // How far the reader has seen is theirs, and is kept where they read.
  'POST /documents/{id}/seen': () => NOTHING,
  'POST /documents/{id}/versions/{version}/restore': (p) => doc(p('id')),
  'POST /documents/{id}/discussions': (p) => doc(p('id')),
  'POST /documents/{id}/discussions/{discussion_id}/comments': (p) => doc(p('id')),
  'POST /documents/{id}/discussions/{discussion_id}/resolve': (p) => doc(p('id')),
  'PATCH /documents/{id}/comments/{comment_id}': (p) => doc(p('id')),
  'DELETE /documents/{id}/comments/{comment_id}': (p) => doc(p('id')),
  'PUT /documents/{id}/comments/{comment_id}/reaction': (p) => [blocks(p('id'))],
  'PUT /documents/{id}/subscription': (p) => [blocks(p('id')), one(p('id'), 'document')],
  'POST /documents/{id}/files': (p) => [blocks(p('id')), one(p('id'), 'document')],
  'DELETE /documents/{id}/files/{file_id}': (p) => [blocks(p('id')), one(p('id'), 'document')],

  // -- uploads: nothing is shown of an upload until it is attached
  'POST /uploads/sessions': () => NOTHING,
  'PUT /uploads/sessions/{upload_id}': () => NOTHING,
  'POST /uploads/sessions/{upload_id}/complete': () => NOTHING,
  'DELETE /uploads/sessions/{upload_id}': () => NOTHING,
  'DELETE /uploads/blobs/{hash}': () => NOTHING,

  // -- exports
  'POST /nodes/{id}/exports/preflight': () => NOTHING,
  'POST /nodes/{id}/exports': () => kinds('exports', 'job'),
  'DELETE /exports/{id}': (p) => [gone(p('id'), 'exports'), every('exports')],

  // -- imports: planning and previewing change nothing; the accepted jobs update their own records
  'POST /nodes/{id}/imports/preflight': () => NOTHING,
  'POST /nodes/{id}/imports/preview': () => NOTHING,
  'POST /nodes/{id}/imports': () => kinds('imports', 'job'),
  'POST /imports/{id}/undo/preflight': () => NOTHING,
  'POST /imports/{id}/undo': (p) => [...ones(p('id'), 'imports'), ...kinds('job')],

  // -- environments and what is in them
  'POST /environments': () => [...kinds('environments', 'start-options', 'hosts', 'host', 'host-runtimes', 'job', 'first-steps'), TREES],
  'POST /machines/{id}/environments': (p) => [...machine(p('id')), ...kinds('environments', 'start-options', 'first-steps')],
  'PATCH /environments/{id}': (p) => [...environment(p('id')), ...settings()],
  'DELETE /environments/{id}': (p) => [...ended(environment(p('id')), p('id')), ...kinds('trace-deleted', 'trace'), TREES],
  'POST /environments/{id}/restore': (p) => [...environment(p('id')), ...kinds('trace-deleted', 'trace', 'job')],
  'DELETE /environments/{id}/purge': (p) => [...environment(p('id')), ...kinds('trace-deleted', 'trace', 'job')],
  'POST /environments/{id}/enable': (p) => environment(p('id')),
  'POST /environments/{id}/disable': (p) => environment(p('id')),
  'POST /environments/{id}/start': (p) => [...environment(p('id')), ...kinds('job', 'thread-parameters')],
  'POST /environments/{id}/stop': (p) => [...environment(p('id')), ...kinds('job', 'thread-parameters')],
  'POST /environments/{id}/restart': (p) => [...environment(p('id')), ...ones(p('id'), 'environment-browsers', 'environment-displays'), ...kinds('job', 'thread-parameters', 'terminals')],
  'POST /environments/{id}/keep-awake': (p) => environment(p('id')),
  'POST /environments/{id}/keep': (p) => environment(p('id')),
  'POST /environments/{id}/files/export': () => kinds('job'),
  'POST /environments/{id}/mcp/servers/{name}/restart': (p) => [one(p('id'), 'environment-mcp'), ...kinds('job', 'trace')],
  'PUT /environments/{id}/config-trust/{harness}': (p) => [...environment(p('id')), every('thread-parameters')],
  'POST /environments/updates/preflight': () => NOTHING,
  'POST /environments/updates': () => placement(),
  'POST /environments/{id}/updates/preflight': () => NOTHING,
  'POST /environments/{id}/updates': (p) => [...environment(p('id')), ...placement()],
  'POST /loss/preflight': () => NOTHING,
  'POST /loss/recovery': () => NOTHING,
  'POST /loss': () => [...placement(), ...threads(), ...kinds('machines','machine','host','hosts','mount','mounts','mcp-server','mcp-servers','providers','provider'), ...inbox()],
  'POST /environments/recover/preflight': () => NOTHING,
  'POST /environments/recover': () => [...placement(), ...threads(), ...kinds('recovery-batch','job'), ...inbox()],
  'POST /hosts/{id}/delete/preflight': () => NOTHING,
  'POST /hosts/{id}/delete': (p) => [...host(p('id')), ...placement(), ...kinds('job'), ...inbox()],
  'POST /environments/{id}/moves/preflight': () => NOTHING,
  'POST /environments/{id}/moves': (p) => [...environment(p('id')), ...placement()],
  'POST /nodes/{id}/environments/updates/preflight': () => NOTHING,
  'POST /nodes/{id}/environments/updates': () => placement(),
  'POST /nodes/{id}/environments/moves/preflight': () => NOTHING,
  'POST /nodes/{id}/environments/moves': () => placement(),
  'POST /nodes/{id}/environments/recycle': () => placement(),
  'POST /hosts/{id}/recycle': (p) => [...host(p('id')), ...placement()],
  'POST /environments/{id}/processes/{pid}/signal': (p) => [one(p('id'), 'processes'), one(p('id'), 'resources')],
  'POST /environments/{id}/terminals': (p) => [one(p('id'), 'terminals')],
  'DELETE /terminals/{terminal_id}': () => kinds('terminals'),
  'POST /terminals/{terminal_id}/ticket': () => NOTHING,
  'POST /environments/{id}/browsers': (p) => ones(p('id'), 'environment-browsers', AGENT_REACH),
  'DELETE /browsers/{browser_id}': () => kinds('environment-browsers'),
  'POST /browsers/{browser_id}/ticket': () => NOTHING,
  'POST /environments/{id}/browser-profiles': (p) => ones(p('id'), 'environment-browsers', AGENT_REACH),
  'PATCH /environments/{id}/browser-profiles/{name}': (p) => ones(p('id'), 'environment-browsers', AGENT_REACH),
  'DELETE /environments/{id}/browser-profiles/{name}': (p) => ones(p('id'), 'environment-browsers', AGENT_REACH),
  'POST /environments/{id}/displays': (p) => [one(p('id'), 'environment-displays')],
  'DELETE /displays/{display_id}': () => kinds('environment-displays'),
  'POST /displays/{display_id}/ticket': () => NOTHING,

  // -- the files of a workspace, its repositories and the states recorded of it
  'PUT /environments/{id}/files/content': (p) => files(p('id')),
  'POST /environments/{id}/files/uploads/{upload_id}/write': () => kinds('job'),
  'POST /environments/{id}/files/uploads/{upload_id}/finish': () => kinds('page-history'),
  'POST /environments/{id}/files/mkdir': (p) => files(p('id')),
  'POST /environments/{id}/files/rename': (p) => files(p('id')),
  'POST /environments/{id}/files/copy': (p) => files(p('id')),
  'DELETE /environments/{id}/files': (p) => files(p('id')),
  'POST /environments/{id}/files/fetch': () => NOTHING,
  'POST /environments/{id}/git/commit': (p) => [...files(p('id')), every('thread')],
  'POST /environments/{id}/git/push': (p) => [...files(p('id')), every('thread')],
  'POST /environments/{id}/restores/preflight': () => NOTHING,
  'POST /environments/{id}/restores': (p) => [...files(p('id')), ...kinds('job', 'source-snapshot', 'restore-plan')],
  'POST /environments/{id}/source/prepare': (p) => [...environment(p('id')), ...files(p('id')), ...kinds('job', 'thread-parameters')],

  // -- ports: openings, their programs, who is let in, and the host's own ports
  'POST /environments/{id}/ports/{port}/openings/preflight': () => NOTHING,
  'POST /environments/{id}/ports/{port}/openings': (p) => [...ones(p('id'), 'environment-ports', 'port-connections', 'port-connection-facts'), every('outside-port-openings')],
  'DELETE /port-openings/{id}': () => kinds('environment-ports', 'port-connections', 'port-connection-facts', 'outside-port-openings'),
  'POST /environments/{id}/ports/{port}/enter': () => NOTHING,
  'PATCH /environments/{id}/ports/{port}/program': (p) => [one(p('id'), 'environment-ports')],
  'PUT /environments/{id}/ports/capabilities/{key}': (p) => [...environment(p('id')), every('port-requests')],
  'DELETE /environments/{id}/capabilities/{key}': (p) => [...environment(p('id')), every('port-requests')],

  // -- providers and how agents run on them
  'POST /providers': () => [...kinds('providers', 'provider-setup', 'start-options', 'first-steps'), TREES],
  'POST /machines/{id}/providers': (p) => [...machine(p('id')), ...kinds('providers', 'provider-setup', 'start-options', 'first-steps')],
  'PATCH /providers/{id}': (p) => [...provider(p('id')), ...settings()],
  'DELETE /providers/{id}': (p) => [...ended(provider(p('id')), p('id')), every('thread-parameters')],
  'POST /providers/{id}/enable': (p) => provider(p('id')),
  'POST /providers/{id}/disable': (p) => provider(p('id')),
  'PUT /providers/{id}/key': (p) => provider(p('id')),
  'PUT /providers/{id}/secrets/{name}': (p) => provider(p('id')),
  'PUT /providers/{id}/facts-overrides': (p) => provider(p('id')),
  'PUT /providers/{id}/sources': (p) => provider(p('id')),
  'POST /providers/{id}/refresh': (p) => provider(p('id')),
  'POST /providers/{id}/harnesses/{agent_id}/restart': (p) => [...provider(p('id')), ...agent(p('agent_id'))],
  'POST /providers/{id}/models': (p) => provider(p('id')),
  'PATCH /providers/{id}/models/{model_id}': (p) => provider(p('id')),
  'DELETE /providers/{id}/models/{model_id}': (p) => provider(p('id')),
  'POST /providers/{id}/models/suggestions/accept': (p) => provider(p('id')),
  'POST /providers/{id}/login': (p) => provider(p('id')),
  'POST /providers/{id}/login/input': (p) => provider(p('id')),
  'POST /providers/{id}/login/cancel': (p) => provider(p('id')),
  'POST /providers/{id}/resets/consume': (p) => [...provider(p('id')), ...usage()],
  'POST /providers/{id}/start': (p) => [...provider(p('id')), ...kinds('host-runtimes', 'job')],
  'POST /providers/{id}/stop': (p) => [...provider(p('id')), ...kinds('host-runtimes', 'job')],
  'POST /providers/{id}/runtime/updates/preflight': () => NOTHING,
  'POST /providers/{id}/runtime/updates': (p) => [...provider(p('id')), ...placement()],
  'POST /providers/runtime/updates/preflight': () => NOTHING,
  'POST /providers/runtime/updates': () => placement(),

  // -- agent profiles
  'POST /orgs/{id}/agents': () => [...kinds('agents', 'start-options', 'first-steps'), TREES],
  'POST /machines/{id}/agents': (p) => [...machine(p('id')), ...kinds('agents', 'start-options', 'first-steps')],
  'PATCH /agents/{id}': (p) => agent(p('id')),
  'DELETE /agents/{id}': (p) => [gone(p('id'), 'agent'), ...kinds('agents', 'start-options', 'thread-parameters')],

  // -- machines a person connected
  'POST /machines/enrollment-tokens': () => kinds('machines'),
  'POST /machines/{id}/enrollment-tokens': (p) => machine(p('id')),
  'PATCH /machines/{id}': (p) => machine(p('id')),
  'PUT /machines/{id}/roles': () => NOTHING,
  'PATCH /machines/{id}/policy': (p) => [...machine(p('id')), ...kinds('environments', 'environment')],
  'POST /machines/{id}/programs': (p) => [...ones(p('id'),'machine-programs'),...machine(p('id'))],
  'POST /machines/{id}/probe': (p) => machine(p('id')),
  'POST /machines/{id}/update': (p) => [...machine(p('id')), ...kinds('hosts', 'host')],
  'POST /hosts/{id}/update': (p) => [...host(p('id')), ...kinds('machines', 'machine')],
  'POST /machines/{id}/revoke': (p) => [...machine(p('id')), ...kinds('environment', 'provider', 'agent', 'thread-parameters')],

  // -- hosts and templates
  'POST /hosts/enrollment-tokens': () => kinds('hosts'),
  'PATCH /hosts/{id}': (p) => host(p('id')),
  'PUT /hosts/{id}/settings/{key}': (p) => host(p('id')),
  'DELETE /hosts/{id}/settings/{key}': (p) => host(p('id')),
  'DELETE /hosts/{id}': (p) => [gone(p('id'), 'host'), gone(p('id'), 'host-runtimes'), ...kinds('hosts', 'machines', 'environments', 'admin', 'start-options')],
  'POST /hosts/{id}/runtimes/{runtime_id}/stop': (p) => [...host(p('id')), ...kinds('provider', 'environment', 'job')],
  'DELETE /hosts/{id}/runtimes/{runtime_id}': (p) => [...host(p('id')), ...kinds('provider', 'environment', 'environments', 'providers')],
  'POST /templates': () => kinds('templates', 'start-options', 'provider-setup'),
  'POST /templates/check': () => NOTHING,
  'PATCH /templates/{id}': (p) => template(p('id')),
  'DELETE /templates/{id}': (p) => [gone(p('id'), 'template'), ...kinds('templates', 'start-options', 'provider-setup')],

  // -- tools: MCP servers and mounts
  'POST /mcp-servers': () => kinds('mcp-servers'),
  'PATCH /mcp-servers/{id}': (p) => mcp(p('id')),
  'POST /mcp-servers/{id}/offerings/{machine_id}/use': (p) => [...mcp(p('id')), ...kinds('environment-mcp')],
  'POST /mcp-servers/{id}/reimport': (p) => mcp(p('id')),
  'DELETE /mcp-servers/{id}': (p) => [gone(p('id'), 'mcp-server'), every('mcp-servers')],
  'POST /mounts': () => kinds('mounts'),
  'PATCH /mounts/{id}': (p) => mount(p('id')),
  'PUT /mounts/{id}/locations/{machine_id}': (p) => mount(p('id')),
  'PATCH /mounts/{id}/locations/{machine_id}': (p) => mount(p('id')),
  'DELETE /mounts/{id}/locations/{machine_id}': (p) => mount(p('id')),
  'DELETE /mounts/{id}/kept/{kept_id}': (p) => mount(p('id')),
  'DELETE /mounts/{id}': (p) => [gone(p('id'), 'mount'), gone(p('id'), 'mount-uses'), every('mounts')],

  // -- skills: the resource, its versions and its draft
  'POST /skills': () => kinds('skills'),
  'PATCH /skills/{id}': (p) => skill(p('id')),
  'DELETE /skills/{id}': (p) => [gone(p('id'), 'skill'), gone(p('id'), 'skill-versions'), gone(p('id'), 'skill-draft'), every('skills')],
  'POST /skills/{id}/versions': (p) => skill(p('id')),
  'POST /skills/{id}/reimport': (p) => skill(p('id')),
  'POST /skills/{id}/draft': (p) => skill(p('id')),
  'DELETE /skills/{id}/draft': (p) => skill(p('id')),
  'PUT /skills/{id}/draft/file': (p) => skill(p('id')),
  'DELETE /skills/{id}/draft/file': (p) => skill(p('id')),
  'POST /skills/{id}/draft/publish': (p) => skill(p('id')),

  // -- usage
  'POST /usage/caps': () => usage(),
  'PATCH /usage/caps/{id}': () => usage(),
  'DELETE /usage/caps/{id}': () => usage(),
  'POST /admin/usage/catalogue': () => usage(),

  // -- the system's management
  'POST /admin/releases/check': () => [only('admin', 'installation-records')],
  'POST /admin/users': () => kinds('admin', 'trace-users'),
  'PATCH /admin/users/{id}': () => [...kinds('admin', 'trace-users', 'trace-person'), ...people()],
  'POST /admin/users/{id}/reset-link': () => NOTHING,
  'POST /admin/users/{id}/org-invitation': () => kinds('admin'),
  'DELETE /admin/users/{id}/org-invitation': () => kinds('admin'),
  'PATCH /admin/orgs/{id}': (p) => [...kinds('admin'), ...place(p('id'))],
  'PUT /admin/capabilities/{key}': () => kinds('admin', 'capabilities', 'drivers', 'template', 'environment'),
  'DELETE /admin/capabilities/{key}': () => kinds('admin', 'capabilities', 'drivers', 'template', 'environment'),
  'PUT /admin/harnesses/{kind}/{version}': () => kinds('admin', 'harness-kinds', 'agent', 'agents'),
  // Program dialogs refresh their captured detail and build page after accepting the native result.
  'PUT /admin/programs/{kind}/builds/{version}': () => NOTHING,
  'POST /admin/programs/{kind}/builds/{version}/check': () => NOTHING,
  'POST /admin/programs/{kind}/builds/{version}/fetch': () => NOTHING,
  'POST /admin/programs/{kind}/channels': () => NOTHING,
  'POST /harnesses/{kind}/{version}/fetch': () => [only('program-builds'), ...kinds('admin', 'harness-kinds')],
  'POST /admin/harnesses/channels': () => kinds('admin', 'harness-kinds'),
  'POST /admin/harnesses/{kind}/{version}/check': () => kinds('admin', 'harness-kinds'),
  'POST /admin/agents': () => kinds('admin', 'agents', 'start-options'),
  'POST /admin/identity-providers': () => kinds('admin', 'meta'),
  'PATCH /admin/identity-providers/{id}': () => kinds('admin', 'meta'),
  'DELETE /admin/identity-providers/{id}': () => kinds('admin', 'meta'),
  // A test of a provider's sign-in says how it went, and the list says when each was last tried.
  'POST /admin/identity-providers/{id}/test': () => kinds('admin'),
  // A rebuild in place changes only the page of search by meaning; the provider it makes is a provider like any.
  'POST /admin/semantic-search/{id}/rebuild': () => [...kinds('admin'), { relatedViews: true }],
  'POST /admin/semantic-search/{id}/keep': (p) => [...kinds('admin'), { relatedViews: true }, ...ones(p('id'), 'settings')],
};

// ---------------------------------------------------------------- finding an act's entry

interface Compiled {
  act: string;
  method: string;
  pattern: RegExp;
  names: readonly string[];
  /** How many parts of the path are written out: the more, the more particular, and the earlier it is tried. */
  literals: number;
  entry: Entry;
}

let compiled: readonly Compiled[] | null = null;

const literal = (text: string): string => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

function compile(): readonly Compiled[] {
  compiled ??= Object.entries(ACTS)
    .map(([act, entry]): Compiled => {
      const space = act.indexOf(' ');
      const segments = act.slice(space + 1).split('/').filter((segment) => segment !== '');
      const names: string[] = [];
      const source = segments
        .map((segment) => {
          const named = /^\{(\w+)\}$/.exec(segment);
          if (named === null) return literal(segment);
          names.push(named[1] ?? '');
          return '([^/]+)';
        })
        .join('/');
      return { act, method: act.slice(0, space), pattern: new RegExp(`^/${source}/?$`), names, literals: segments.length - names.length, entry };
    })
    .sort((first, second) => second.literals - first.literals);
  return compiled;
}

const decode = (part: string): string => {
  try {
    return decodeURIComponent(part);
  } catch {
    return part;
  }
};

/** The entry of a request, by its method and its path below the API's base; `null` for one the registry does not know. */
export function findAct(method: string, path: string, payload: Pick<Act, 'input' | 'answer'> = {}): { act: string; changes: readonly Change[] } | null {
  for (const candidate of compile()) {
    if (candidate.method !== method) continue;
    const found = candidate.pattern.exec(path);
    if (found === null) continue;
    const params = new Map(candidate.names.map((name, index) => [name, decode(found[index + 1] ?? '')]));
    const p: PathParam = (name) => params.get(name) ?? '';
    return { act: candidate.act, changes: answerChanges({ method: method as Act['method'], path, ...payload }, p, candidate.entry(p)) };
  }
  return null;
}

// ---------------------------------------------------------------- applying a change

/** How deep an identifier may stand in a key: `[kind, which one, how asked]`, where "how asked" may be an object of a few members. */
const KEY_DEPTH = 4;

function names(part: unknown, id: string, depth: number): boolean {
  if (part === id) return true;
  if (depth >= KEY_DEPTH) return false;
  if (Array.isArray(part)) return part.some((inner) => names(inner, id, depth + 1));
  if (typeof part === 'object' && part !== null) return Object.values(part).some((inner) => names(inner, id, depth + 1));
  return false;
}

/** Whether a query's key is of the data a change names. */
export function covers(change: Stale, key: QueryKey): boolean {
  if (key.length < change.key.length) return false;
  if (change.exact === true && key.length !== change.key.length) return false;
  for (const [index, part] of change.key.entries()) if (key[index] !== part) return false;
  if (change.at !== undefined && key[change.at.index] !== change.at.value) return false;
  return change.id === undefined || names(key, change.id, 0);
}

/** What keeps the data a live feed follows: set by the live connection when it starts. Until then those changes wait for the feed itself. */
export interface Followers {
  /** Retakes the same human session and drops old-authority live projections before reloading them. */
  session?: () => void;
  trees: () => void;
  thread: (id: string) => void;
  document: (id: string) => void;
  counters: () => void;
}

let followers: Followers | null = null;

/** Tells the registry how to ask the live feeds for what changed. */
export function followWith(given: Followers): void {
  followers = given;
}

function follow(change: Followed): void {
  const given = followers;
  if (given === null) return;
  switch (change.feed) {
    case 'trees':
      given.trees();
      return;
    case 'counters':
      given.counters();
      return;
    case 'thread':
      if (change.id !== undefined) given.thread(change.id);
      return;
    case 'document':
      if (change.id !== undefined) given.document(change.id);
      return;
  }
}

const startRepairs = new WeakMap<Query, Promise<void>>();
const approvalRepairs = new WeakMap<Query, Promise<void>>();
const relatedRepairs = new WeakMap<Query, Promise<void>>();

/** Ranking and authority can change without the source document's body changing. Refresh only displayed lists. */
async function repairRelatedViews(cache: QueryClient, current: () => boolean): Promise<void> {
  await Promise.all(cache.getQueryCache().getAll().filter((query) => query.queryKey[0] === 'document-related').map(async (query) => {
    if (!query.isActive()) { cache.removeQueries({ queryKey: query.queryKey, exact: true }); return; }
    const pending = relatedRepairs.get(query);
    if (pending !== undefined) return pending;
    const repair = (async () => {
      await cache.cancelQueries({ queryKey: query.queryKey, exact: true });
      if (!current() || cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) !== query || !query.isActive() || !queryAuthorityAllows(cache, query.meta)) return;
      // Old continuation pages must disappear before rechecking access or ranking.
      await cache.resetQueries({ queryKey: query.queryKey, exact: true });
    })();
    relatedRepairs.set(query, repair);
    return repair.finally(() => { if (relatedRepairs.get(query) === repair) relatedRepairs.delete(query); });
  }));
}

/** A saved rate setting cannot supply model-specific resolution, provider precedence or estimates. */
async function repairRateViews(cache: QueryClient, nodeId: string | null, current: () => boolean): Promise<void> {
  const affected = cache.getQueryCache().getAll().filter((query) => {
    const [head, part, place] = query.queryKey;
    if (head !== 'usage' || part !== 'rates') return false;
    if (nodeId === null || place === nodeId) return true;
    if (typeof place !== 'string') return false;
    const node = cache.getQueryData(['node', place]);
    return record(node) && Array.isArray(node.path) && node.path.some((part: unknown) => record(part) && part.id === nodeId);
  });
  await Promise.all(affected.map(async (query) => {
    if (!query.isActive()) { cache.removeQueries({ queryKey: query.queryKey, exact: true }); return; }
    await cache.cancelQueries({ queryKey: query.queryKey, exact: true });
    if (!current() || cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) !== query
      || !query.isActive() || !queryAuthorityAllows(cache, query.meta)) return;
    await cache.refetchQueries({ queryKey: query.queryKey, exact: true });
  }));
}

function capIn(value: unknown, id: string): Record<string, unknown> | undefined {
  if (Array.isArray(value)) {
    for (const row of value as unknown[]) { const found = capIn(row, id); if (found !== undefined) return found; }
  } else if (record(value)) {
    if (value.id === id && 'node' in value && 'provider' in value) return value;
    if (record(value.cap) && value.cap.id === id) return value.cap;
    if (Array.isArray(value.caps)) return capIn(value.caps, id);
  }
  return undefined;
}

/** A cap definition omits use, inherited applicability and the reader's share; their native owners supply those facts. */
async function repairCapViews(cache: QueryClient, change: { id: string; value?: Record<string, unknown> }, current: () => boolean): Promise<void> {
  const queries = cache.getQueryCache().getAll();
  const native = change.value ?? queries.map((query) => capIn(query.state.data, change.id)).find((value) => value !== undefined);
  const at = native?.node, using = native?.provider;
  const nodeId = at === null ? null : record(at) && typeof at.id === 'string' ? at.id : undefined;
  const providerId = using === null ? null : record(using) && typeof using.id === 'string' ? using.id : undefined;
  const coversNode = (id: string): boolean => {
    if (nodeId === null || nodeId === id) return true;
    const node = cache.getQueryData(['node', id]) ?? cache.getQueryData(['thread', id]);
    return record(node) && Array.isArray(node.path) && node.path.some((part: unknown) => record(part) && part.id === nodeId);
  };
  const usesProvider = (id: string): boolean => {
    const thread = cache.getQueryData(['thread', id]) ?? cache.getQueryData(['node', id]);
    const binding = record(thread) && record(thread.binding) ? thread.binding : undefined;
    const provider = binding?.provider;
    return record(provider) && provider.id === providerId;
  };
  const affected = queries.filter((query) => {
    const [head, part, place] = query.queryKey;
    if (head !== 'thread-limits' && head !== 'provider-caps' && (head !== 'usage' || !['caps', 'limits', 'mine'].includes(String(part)))) return false;
    if (capIn(query.state.data, change.id) !== undefined) return true;
    if (nodeId === undefined || providerId === undefined) return false;
    if (head === 'provider-caps') return providerId !== null && part === providerId;
    if (head === 'thread-limits') return typeof part === 'string' && (providerId === null ? coversNode(part) : usesProvider(part));
    if (part === 'mine') return native?.split === 'each';
    if (part === 'limits') return typeof place === 'string' && (providerId === null ? coversNode(place) : usesProvider(place));
    if (place === 'system') return nodeId === null && providerId === null;
    if (typeof place !== 'string') return false;
    if (place === `provider:${providerId}`) return providerId !== null;
    const node = /^node:(.+):(true|false)$/u.exec(place);
    return providerId === null && node !== null && (node[1] === nodeId || node[2] === 'true' && node[1] !== undefined && coversNode(node[1]));
  });
  await Promise.all(affected.map(async (query) => {
    if (!query.isActive()) { cache.removeQueries({ queryKey: query.queryKey, exact: true }); return; }
    await cache.cancelQueries({ queryKey: query.queryKey, exact: true });
    if (!current() || cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) !== query
      || !query.isActive() || !queryAuthorityAllows(cache, query.meta)) return;
    await cache.refetchQueries({ queryKey: query.queryKey, exact: true });
  }));
}

/** Approval news carries counters, without the request or its membership in the reader's lists. */
async function repairApprovalViews(cache: QueryClient, current: () => boolean): Promise<void> {
  await Promise.all(cache.getQueryCache().getAll().filter((query) => query.queryKey[0] === 'approvals' || query.queryKey[0] === 'approval' || query.queryKey[0] === 'reference-preview' || query.queryKey[0] === 'access-request').map(async (query) => {
    if (!query.isActive()) { cache.removeQueries({ queryKey: query.queryKey, exact: true }); return; }
    const pending = approvalRepairs.get(query);
    if (pending !== undefined) return pending;
    const repair = (async () => {
      await cache.cancelQueries({ queryKey: query.queryKey, exact: true });
      if (!current() || cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) !== query
        || !query.isActive() || !queryAuthorityAllows(cache, query.meta)) return;
      await cache.refetchQueries({ queryKey: query.queryKey, exact: true });
    })();
    approvalRepairs.set(query, repair);
    return repair.finally(() => { if (approvalRepairs.get(query) === repair) approvalRepairs.delete(query); });
  }));
}

/** A newly reported agent changes computed arrangements, which its native row cannot supply. */
async function repairAgentChoices(cache: QueryClient, row: RowChange['row'], current: () => boolean): Promise<void> {
  if (row.kind !== 'agent' || row.value === null || !record(row.value.descriptor)) return;
  const reported = Object.keys(row.value.descriptor).length > 0;
  const waiting = cache.getQueryCache().getAll().filter((query) => {
    if (query.queryKey[0] !== 'start-options') return false;
    const choices = query.state.data as BindingOptions | undefined;
    return choices?.providers.some((provider) => provider.agents.some((agent) => agent.agent.id === row.id
      && (reported ? agent.reason === 'harness_unreported' : agent.reason !== 'harness_unreported' && agent.reason !== 'disabled'))) === true;
  });
  await Promise.all(waiting.map(async (query) => {
    if (!query.isActive()) { cache.removeQueries({ queryKey: query.queryKey, exact: true }); return; }
    const pending = startRepairs.get(query);
    if (pending !== undefined) return pending;
    const repair = (async () => {
      await cache.cancelQueries({ queryKey: query.queryKey, exact: true });
      if (!current() || cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) !== query
        || !query.isActive() || !queryAuthorityAllows(cache, query.meta)) return;
      await cache.refetchQueries({ queryKey: query.queryKey, exact: true });
    })();
    startRepairs.set(query, repair);
    return repair.finally(() => { if (startRepairs.get(query) === repair) startRepairs.delete(query); });
  }));
}

/**
 * Applies native answers and frames in place. Missing projections repair one exact visible query;
 * data not carried by an answer is never reconstructed from a guessed permission or a local rate.
 */
export async function applyChanges(changes: readonly Change[], cache: QueryClient = queryClient): Promise<void> {
  const authority=changes.find((change):change is SessionAuthorityChanged=>'session' in change);
  if (authority!==undefined) {
    const held=cache.getQueryData(['admin-mode']);
    if(record(authority.person)) cache.setQueryData(['me'], authority.person);
    if(record(authority.mode)) cache.setQueryData(['admin-mode'],authority.mode);
    else if(record(held) && authority.asAdmin!==undefined) cache.setQueryData(['admin-mode'],{...held,as_admin:authority.asAdmin});
    const readerVersion = (client.session.getState().readerVersion ?? 0) + 1;
    client.session.setState({ readerVersion });
    await cache.cancelQueries();
    if (client.session.getState().readerVersion !== readerVersion) return;
    // A mode answer cannot carry fresh permission cuts for arbitrary open objects. Drop old
    // authority immediately and repair only exact visible projections. Personal preferences,
    // the person and release catalogues do not change, and unopened caches cause no reads.
    const personal=new Set(['me','me-state','admin-mode','chat-states','notification-prefs','settings-registry','harness-kinds','drivers','capabilities','shell-bootstrap']);
    const repair:QueryKey[]=[];
    for(const query of cache.getQueryCache().getAll()) {
      if(typeof query.queryKey[0]==='string' && personal.has(query.queryKey[0])) continue;
      // Mode-only suppliers declare their authority even when their keys are shared with
      // ordinary scopes. Old mounted observers cannot authorize a replacement read.
      if(!query.isActive() || !queryAuthorityAllows(cache, query.meta)) cache.removeQueries({queryKey:query.queryKey,exact:true});
      else {query.setState({data:undefined,error:null,status:'pending',fetchStatus:'idle'});repair.push(query.queryKey);}
    }
    followers?.session?.();
    await Promise.all(repair.map((queryKey)=>cache.refetchQueries({queryKey,exact:true})));
    return;
  }
  const stale: Stale[] = [];
  const directoryHints = changes.filter((change): change is Extract<Change, { people: true }> => 'people' in change);
  const directoryCut = directoryHints.length === 0 ? undefined : Math.max(...directoryHints.map((change) => change.cut ?? capturePeopleHint(cache)));
  const readerVersion = client.session.getState().readerVersion;
  const left: Stale[] = [];
  // Each change is taken once, however many entries or frames name it: a moved subtree names the same kinds for every row.
  const seen = new Set<string>();
  let metaApplied = false;
  for (const change of changes) {
    if (client.session.getState().readerVersion !== readerVersion) return;
    if ('session' in change) continue;
    const name = 'people' in change ? 'people' : JSON.stringify(change);
    if (seen.has(name)) continue;
    seen.add(name);
    const adminKey = 'collection' in change ? change.collection.key : 'value' in change ? change.value.key : null;
    if (adminKey?.[0] === 'admin') {
      await cache.cancelQueries({ queryKey: adminKey, exact: true });
      if (client.session.getState().readerVersion !== readerVersion) return;
    }
    if ('adminOrgRestore' in change) await repairAdminOrgs(cache, () => client.session.getState().readerVersion === readerVersion);
    else if ('adminUser' in change) await applyAdminUser(cache, change.adminUser, () => client.session.getState().readerVersion === readerVersion);
    else if ('meta' in change) {
      if (!metaApplied) {
        metaApplied = true;
        const causes = changes.flatMap((item) => 'meta' in item ? [item.requestId] : []);
        await repairMeta(cache, () => client.session.getState().readerVersion === readerVersion, causes);
      }
    }
    else if ('thread' in change) applyThread(cache, change.thread);
    else if ('row' in change) {
      const { kind, id } = change.row;
      if (kind === 'machine' || kind === 'host') {
        const pending = cache.getQueryCache().getAll().filter((query) => {
          const data = query.state.data;
          const row = record(data) ? data[kind] : null;
          return query.queryKey[0] === ENROLLMENT_PROGRESS && query.state.fetchStatus === 'fetching' && record(row) && row.id === id;
        });
        if (pending.length > 0) {
          await Promise.all(pending.map((query) => cache.cancelQueries({ queryKey: query.queryKey, exact: true })));
          if (client.session.getState().readerVersion !== readerVersion) return;
        }
      }
      const projection=missingInventoryProjection(cache,change.row);
      applyRow(cache, change.row);
      if(projection!==null) await cache.refetchQueries({queryKey:projection,exact:true});
      if (client.session.getState().readerVersion !== readerVersion) return;
      await repairAgentChoices(cache, change.row, () => client.session.getState().readerVersion === readerVersion);
    }
    else if ('rateViews' in change) await repairRateViews(cache, change.rateViews.nodeId, () => client.session.getState().readerVersion === readerVersion);
    else if ('capViews' in change) await repairCapViews(cache, change.capViews, () => client.session.getState().readerVersion === readerVersion);
    else if ('approvalViews' in change) await repairApprovalViews(cache, () => client.session.getState().readerVersion === readerVersion);
    else if ('people' in change) {
      let supplied: readonly QueryKey[] = [];
      if (directoryHints.some((hint) => hint.authority === true)) {
        const baseline = await readAuthority(cache);
        if (baseline === null || client.session.getState().readerVersion !== readerVersion) return;
        if (baseline.changed) {
          await applyChanges([{ session: true, mode: baseline.mode, person: baseline.person }], cache);
          return;
        }
        supplied = baseline.supplied;
      }
      await repairPeople(cache, supplied, directoryCut);
    }
    else if ('relatedViews' in change) await repairRelatedViews(cache, () => client.session.getState().readerVersion === readerVersion);
    else if ('documentTree' in change) await applyDocumentTreeChange(cache, change.documentTree);
    else if ('documentDirectories' in change) await repairDocumentDirectories(cache, change.documentDirectories);
    else if ('missing' in change) {
      // A native live view repairs itself from its feed; a legacy hint must not evict or read it.
      const candidates=cache.getQueryCache().getAll().filter((query)=>queryFeed(query.queryKey)===null
        && change.missing.some((candidate)=>covers(candidate,query.queryKey))
        && (change.containingRow===undefined || Array.isArray(query.state.data)
          && (query.state.data as unknown[]).some((row)=>record(row) && row.id===change.containingRow))
        && (change.containingAccount===undefined || Array.isArray(query.state.data)
          && (query.state.data as unknown[]).some((row)=>record(row) && record(row.user) && row.user.id===change.containingAccount)));
      const query=candidates.find((query)=>query.isActive()) ?? candidates[0];
      if (query?.isActive() && change.cancelPending === true) {
        await cache.cancelQueries({ queryKey: query.queryKey, exact: true });
        if (client.session.getState().readerVersion !== readerVersion
          || cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) !== query) return;
      }
      if(query?.isActive()) await cache.refetchQueries({queryKey:query.queryKey,exact:true});
      else if(query!==undefined) cache.removeQueries({queryKey:query.queryKey,exact:true});
    }
    else if ('repair' in change) await repairResource(cache, change.repair);
    else if ('enrollment' in change) {
      await cache.cancelQueries({ queryKey: enrollmentProgressKey(change.enrollment.receipt), exact: true });
      if (client.session.getState().readerVersion !== readerVersion) return;
      await applyChanges(enrollmentChanges(cache, change.enrollment), cache);
    }
    else if ('agentReach' in change) {
      // The answer omits this independently authorized overview. Repair only native related
      // views actually shown; unopened views expire without a request or a list invalidation.
      for (const key of agentReachProjections(cache, change.agentReach)) {
        const query = cache.getQueryCache().find({ queryKey: key, exact: true });
        if (query?.isActive() !== true) cache.removeQueries({ queryKey: key, exact: true });
        else {
          await cache.cancelQueries({ queryKey: key, exact: true });
          if (client.session.getState().readerVersion !== readerVersion) return;
          await cache.refetchQueries({ queryKey: key, exact: true });
          if (client.session.getState().readerVersion !== readerVersion) return;
        }
      }
    }
    else if ('value' in change) cache.setQueryData(change.value.key, change.value.data);
    else if ('collection' in change) {
      if (change.cancelPending === true) {
        const query = cache.getQueryCache().find({ queryKey: change.collection.key, exact: true });
        await cache.cancelQueries({ queryKey: change.collection.key, exact: true });
        if (client.session.getState().readerVersion !== readerVersion
          || cache.getQueryCache().find({ queryKey: change.collection.key, exact: true }) !== query) return;
      }
      applyCollection(cache, change.collection);
    }
    else if ('state' in change) cache.setQueryData<Record<string, unknown>>(['me-state'], (held) => {
      const before = held?.[change.state.key];
      const data = change.state.child === undefined ? change.state.data : { ...(record(before) ? before : {}), [change.state.child]: change.state.data };
      return { ...held, [change.state.key]: data };
    });
    else if ('feed' in change) follow(change);
    else if (change.gone === true) left.push(change);
    else stale.push(change);
  }
  const isGone = (query: Query): boolean => left.some((change) => covers(change, query.queryKey));
  if (left.length > 0) cache.removeQueries({ predicate: (query) => query.getObserversCount() === 0 && isGone(query) });
  if (stale.length === 0) return;
  // Legacy invalidations received without a row never refresh a whole kind or a list. Exact
  // missing projections are the only reads allowed; inactive entries wait until they are shown.
  const exact = stale.filter((change) => change.exact === true || change.id !== undefined);
  const missing = cache.getQueryCache().getAll().filter((query) => !isGone(query) && query.isActive() && exact.some((change) => covers(change, query.queryKey)));
  await Promise.all(missing.map((query) => cache.invalidateQueries({ queryKey: query.queryKey, exact: true, refetchType: 'active' })));
}

/** What an act whose request the registry does not know is: a fault of the application, said where it is seen. */
function unregistered(act: Act): void {
  console.error(`An act has no entry in the registry of acts (src/api/acts.ts): ${act.method} ${act.path}`);
}

/** What follows an act that succeeded: its entry's changes applied, before its caller goes on. The client tells it of every one. */
export async function actSucceeded(act: Act): Promise<void> {
  if (act.readerVersion !== undefined && act.readerVersion !== (client.session.getState().readerVersion ?? 0)) return;
  const found = findAct(act.method, act.path, act);
  if (found === null) unregistered(act);
  else await applyChanges(found.changes);
}

client.onAct(actSucceeded);

// ---------------------------------------------------------------- what the live connection says

/**
 * What a frame of an org's tree says changed, from the rows it brings and the rows held before it: a place made,
 * renamed, moved, archived or deleted is a row of its parent's lists; a row whose summary is the same but whose
 * sequence moved is a change of its roles, its page or its settings, which reach what is inside it; a thread
 * that changed its state changes the attention of its topic and of the project that lists it.
 */
export function treeChanges(held: ReadonlyMap<string, TreeNode>, rows: readonly TreeNode[]): Change[] {
  const changes: Change[] = [];
  for (const row of rows) {
    const before = held.get(row.id);
    if (before !== undefined && before.tree_seq >= row.tree_seq) continue;
    // Tree rows are summaries. Overlay only those fields in loaded references; the tree store
    // separately applies the complete row. Do not refetch every descendant's page/settings.
    changes.push({ row: { kind: 'node', id: row.id, value: row.deleted ? null : { ...row }, partial: true } });
  }
  return changes;
}

/** What the person's own news says changed, by its kind (`what`) and its members. */
export function userEventChanges(what: string, data: Readonly<Record<string, unknown>>): Change[] {
  if(what==='subscription' && typeof data.node_id==='string' && (data.level===null || data.level==='all' || data.level==='important' || data.level==='off')) {
    const key=['subscription-choice',data.node_id];
    if(queryClient.getQueryData(key)===data.level)return [];
    return [{value:{key,data:data.level}}, {missing:[{key:['notification-exceptions']}]},
      {missing:[{key:['node',data.node_id],exact:true}]}, {feed:'thread',id:data.node_id}];
  }
  const id = (member: string): string | null => {
    const value = data[member];
    return typeof value === 'string' ? value : null;
  };
  const snapshots: Change[] = [];
  for (const [member, key] of [['favorites', 'favorites'], ['recents', 'recents'], ['notification_prefs', 'notification-prefs'], ['channels', 'notification-channels'], ['machines', 'machines'], ['chat_states', 'chat-states']] as const) {
    if (member in data) snapshots.push({ value: { key: [key], data: data[member] } });
  }
  for (const [member, key] of [['routes', ['away', 'routes']], ['devices', ['away', 'devices']], ['summary', ['away', 'summary', null]]] as const) {
    if (member === 'summary' && what !== 'route') continue;
    if (member in data) snapshots.push({ value: { key, data: data[member] } });
  }
  if (snapshots.length > 0) return snapshots;
  switch (what) {
    case 'admin_mode':
      return [{ session: true, ...(typeof data.as_admin==='boolean'?{asAdmin:data.as_admin}:{}) }];
    case 'approvals':
      return record(data.approval) && typeof data.approval.id === 'string' ? [{ row: { kind: 'approval', id: data.approval.id, value: data.approval, insert: true } }] : [{ approvalViews: true }];
    case 'favorites':
      return [];
    case 'recents':
      return [];
    case 'state':
      return typeof data.key === 'string' && 'value' in data ? [{ state: { key: data.key, data: data.value, ...(typeof data.child === 'string' ? { child: data.child } : {}) } }] : [];
    case 'org_creation':
    case 'meta_changed':
      // Native hints contain no Meta DTO; its permission belongs to this exact current reader.
      return [{ meta: true, ...(typeof data.request_id === 'string' && data.request_id !== '' ? { requestId: data.request_id } : {}) }];
    case 'machines': {
      const machineId = id('machine_id');
      return resourceChanges('machine', machineId, data, 'news');
    }
    case 'enrollment': {
      const receipt = id('receipt');
      if (receipt === null) return [];
      const status = data.status;
      if (!record(status)) return [{ missing: [{ key: enrollmentProgressKey(receipt), exact: true }], cancelPending: true }];
      if (status.receipt !== receipt) return [];
      return [{ enrollment: { receipt, status } }];
    }
    case 'resources':
      return resourceChanges(id('kind') ?? '', id('id'), data, 'news');
    case 'connected':
      // Personal hints are not replayed. A reconnect repairs the visible permission cuts missed while offline.
      return data.resumed === true ? [{ meta: true },{approvalViews:true}] : [];
    default:
      return [];
  }
}

/** The lists of each kind of resource, as the person's news names the kind. */
const RESOURCE_LISTS: Readonly<Record<string, readonly string[]>> = {
  provider: ['providers', 'provider-setup', 'start-options'],
  agent: ['agents', 'start-options'],
  host: ['hosts'],
  template: ['templates', 'start-options'],
  mcp_server: ['mcp-servers'],
  mount: ['mounts'],
  skill: ['skills'],
  environment: ['environments', 'start-options'],
  machine: ['machines'],
};

/** What a resource's own feed, or the person's news of one, says changed: that resource as every view reads it. */
export function resourceChanges(kind: string, id: string | null, data: Readonly<Record<string, unknown>> = {}, source?: 'news'): Change[] {
  if (!(kind in RESOURCE_LISTS) || id === null) return [];
  const references:Change[]=kind==='environment'?[{key:['reference-preview','environment_file',id]}]:[];
  if (data.deleted === true || data.view_revoked === true) return [{ row: { kind, id, value: null } },...references];
  let value = data.row ?? data[kind] ?? data.value;
  // Native events use the inventory's kind/value union; older servers may still send a flat row.
  if (record(value) && typeof value.kind === 'string' && record(value.value)) {
    if (value.kind !== kind) return [];
    value = value.value;
  }
  if (record(value) && value.id === id) return [{ row: { kind, id, value, insert: true } },...references];
  // Older/event-only servers may omit the native row. Read only that visible detail, once.
  return [{ repair: { kind, id, ...(source === undefined ? {} : { source }) } },...references];
}
