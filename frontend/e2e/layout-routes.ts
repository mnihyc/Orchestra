/** Every shipped route pattern has a concrete fake-world screen; parameterized sections are expanded. */
import { ACME, id, RUNS_ON, TRACE } from '../tests/doubles/fixtures.ts';
import type { Scenario } from '../tests/doubles/world.ts';
import { TREE } from '../tests/e2e/document-tree-support.ts';

export interface LayoutRoute { pattern: string; url: string; note?: string; scenario?: Scenario; fixture?: 'document-tree' }
const route = (pattern: string, url = pattern): LayoutRoute => ({ pattern, url });
const sections = (pattern: string, prefix: string, names: readonly string[]) => names.map((name) => route(pattern, `${prefix}${name}`));
const tabs = (kind: string, resource: string, names: readonly string[]) => names.map((tab) => route(`/resources/${kind}/$id`, `/resources/${kind}/${resource}?tab=${tab}`));

export const MCP_ID = id(0x31, 1);
export const MOUNT_ID = id(0x32, 1);
export const SKILL_ID = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a62';
export const ORG_SECTIONS = ['general', 'departments', 'members', 'groups', 'roles', 'policies', 'tags', 'service-accounts', 'audit', 'everything', 'channels', 'destinations'];
export const ADMIN_SECTIONS = ['overview', 'users', 'orgs', 'everything', 'sign-in', 'email', 'hosts', 'runtimes', 'templates', 'mcp', 'capabilities', 'harnesses', 'agents', 'plugins', 'defaults', 'rates', 'settings', 'storage', 'semantic-search', 'system', 'pki', 'audit'];

export const SIGNED_IN_ROUTES: LayoutRoute[] = [
  route('/'), route('/admin'), route('/resources'), route('/settings'), route('/workbench'),
  route('/org/$slug', '/org/acme'), route('/org/$slug', '/org/acme.research'), route('/org/$slug', '/org/ada'),
  route('/org/$slug/messages', '/org/acme/messages'),
  route('/chat/$chatId', `/chat/${id(9, 1)}`), route('/place-chat/$nodeId', `/place-chat/${ACME.org}`),
  ...sections('/org/$slug/settings/$section', '/org/acme/settings/', ORG_SECTIONS),
  route('/project/$projectId', `/project/${ACME.web}`), route('/project/$projectId', `/project/${ACME.web}?view=board`),
  ...['general', 'policies', 'tags', 'audit', 'destinations'].map((section) => route('/project/$projectId/settings', `/project/${ACME.web}/settings?section=${section}`)),
  route('/topic/$topicId', `/topic/${ACME.fixLogin}`),
  ...['general', 'policies', 'audit', 'destinations'].map((section) => route('/topic/$topicId', `/topic/${ACME.fixLogin}?tab=settings&section=${section}`)),
  route('/thread/$threadId', `/thread/${ACME.mainThread}`),
  route('/doc/$documentId', `/doc/${id(7, 1)}`),
  route('/documents/$nodeId', `/documents/${ACME.web}`),
  { ...route('/document-folder/$folderId', `/document-folder/${TREE.research}`), fixture: 'document-tree' },
  route('/inspect/$kind/$id', `/inspect/thread/${TRACE.thread}`),
  route('/trace/$nodeId', `/trace/${ACME.mainThread}`), route('/u/$username', '/u/ada'),
  ...['/inbox', '/approvals', '/everything', '/search', '/shared', '/usage', '/help'].map((path) => route(path)),
  route('/help/$article', '/help/finding-things'),
  ...['profile', 'security', 'tokens', 'notifications', 'interface', 'exports', 'usage', 'your-page'].map((section) => route(`/settings/${section}`)),
  ...['providers', 'agents', 'environments', 'hosts', 'machines', 'templates', 'mcp', 'mounts', 'skills'].map((kind) => route(`/resources/${kind}`)),
  ...tabs('providers', RUNS_ON.openAiKey, ['overview', 'models', 'key', 'usage', 'settings', 'activity']),
  ...tabs('agents', RUNS_ON.codex, ['overview', 'parameters', 'profile', 'access']),
  ...tabs('environments', RUNS_ON.workspace, ['overview', 'threads', 'files', 'tools', 'terminal', 'ports', 'browsers', 'vnc', 'limits', 'trust', 'controls', 'access', 'settings', 'danger']),
  ...tabs('hosts', RUNS_ON.buildHost, ['overview', 'runtimes', 'storage', 'settings', 'policy', 'ports', 'access']),
  ...tabs('templates', RUNS_ON.defaultTemplate, ['overview', 'spec', 'access', 'danger']),
  route('/resources/machines/$id', `/resources/machines/${RUNS_ON.laptop}`),
  route('/resources/mcp/$id', `/resources/mcp/${MCP_ID}`), route('/resources/mounts/$id', `/resources/mounts/${MOUNT_ID}`),
  ...tabs('skills', SKILL_ID, ['overview', 'versions', 'draft']),
  ...sections('/admin/$section', '/admin/', ADMIN_SECTIONS),
  // This address is deliberately a not-found screen in the shipped application, not the development gallery.
  { ...route('/dev/ui'), note: 'Production deliberately excludes the shared-control gallery.' },
  { pattern: '(not found)', url: '/no-such-layout-route', note: 'The routed not-found screen is also part of the product.' },
];

export const SIGNED_OUT_ROUTES: LayoutRoute[] = [
  route('/login'), route('/register'), route('/recover'),
  { ...route('/setup'), scenario: { scenario: 'fresh' }, note: 'The first-owner form, on a fresh fake system.' },
  { ...route('/reset/$token', '/reset/no-longer-valid'), note: 'An expired password link has its own actionable screen.' },
  { ...route('/invite/$token', '/invite/no-longer-valid'), note: 'An expired invitation stays a named refusal.' },
  { ...route('/verify-email/$token', '/verify-email/no-longer-valid'), note: 'An expired confirmation stays a named refusal.' },
];

/** Known seed routing/state spellings whose display names differ. Dotted key patterns are checked too. */
export const RAW_VALUES = ['acme', 'acme.research', 'acme.dep2', 'system.Claude Code', 'mock-large', 'on_request', 'waiting_approval', 'gpt-5.4', 'gpt-5.5-codex', 'claude-opus-5'];
