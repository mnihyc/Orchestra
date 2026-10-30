/** The same complete route inventory as the layout gate, with explicitly named reachable review states. */
import { ACME, id, RUNS_ON, TRACE } from '../../tests/doubles/fixtures.ts';
import { MCP_ID, MOUNT_ID, SIGNED_IN_ROUTES, SIGNED_OUT_ROUTES, SKILL_ID } from '../layout-routes.ts';
import { mapRoute, type CaptureConfig, type ClickTarget } from './config.ts';
import { TREE } from '../../tests/e2e/document-tree-support.ts';

export interface CaptureState {
  id: string;
  label: string;
  pattern: string;
  url: string;
  signedIn: boolean;
  kind: 'route' | 'query' | 'container' | 'view' | 'dialog';
  note?: string;
  container?: string;
  view?: string;
  clicks?: ClickTarget[];
  /** Exact dialog accessible name, or any dialog when the name includes world-specific text. */
  dialog?: string;
}

export const THREAD_CONTAINERS = ['conversation', 'team', 'code', 'environment', 'agent'] as const;
export const THREAD_VIEWS = ['outline', 'history', 'chat', 'documents', 'people', 'changes', 'files', 'git', 'environment', 'processes',
  'resources', 'ports', 'browsers', 'displays', 'terminal', 'parameters', 'usage', 'knowledge', 'given', 'diagnostics', 'help'] as const;
const thread = `/thread/${ACME.mainThread}`;
const topic = `/topic/${ACME.fixLogin}`;
const project = `/project/${ACME.web}`;
const machine = `/resources/machines/${RUNS_ON.laptop}`;
/** These are review fixtures, not guesses about which arbitrary object the integrator wanted. */
export const TARGET_SEEDS: Readonly<Record<string, string>> = {
  org: '/org/acme', department: '/org/acme.research', personalOrg: '/org/ada',
  project, topic, thread, document: `/doc/${id(7, 1)}`, inspectedThread: `/inspect/thread/${TRACE.thread}`,
  folder: `/document-folder/${TREE.research}`, documents: `/documents/${ACME.web}`, chat: `/chat/${id(9, 1)}`, placeChat: `/place-chat/${ACME.org}`,
  trace: `/trace/${ACME.mainThread}`, profile: '/u/ada', provider: `/resources/providers/${RUNS_ON.openAiKey}`,
  agent: `/resources/agents/${RUNS_ON.codex}`, environment: `/resources/environments/${RUNS_ON.workspace}`,
  host: `/resources/hosts/${RUNS_ON.buildHost}`, template: `/resources/templates/${RUNS_ON.defaultTemplate}`, machine,
  mcp: `/resources/mcp/${MCP_ID}`, mount: `/resources/mounts/${MOUNT_ID}`, skill: `/resources/skills/${SKILL_ID}`,
};
const click = (name: string, within?: string): ClickTarget => ({ role: 'button', name, ...(within === undefined ? {} : { within }) });

/** These clicks open a form/picker only. No creation, confirmation, shell, browser or process is started. */
const DIALOGS: Omit<CaptureState, 'signedIn' | 'kind' | 'pattern'>[] = [
  { id: 'find', label: 'Find anything', url: thread, clicks: [click('@finder.open', '[data-screen-titlebar]')], dialog: '@finder.title' },
  { id: 'thread-share', label: 'Share thread', url: thread, clicks: [click('@actions.share.label', '[data-thread-header]')], dialog: '*' },
  { id: 'thread-find', label: 'Find in conversation', url: thread, clicks: [click('@search.find.command', '[data-thread-header]')], dialog: '@search.find.label' },
  { id: 'choose-view', label: 'Choose a workbench view', url: thread, view: 'chooser', dialog: '@workbench.openView.choose' },
  { id: 'add-provider', label: 'Add provider', url: '/resources/providers', clicks: [click('@providers.list.add')], dialog: '@providers.add.title' },
  { id: 'add-machine', label: 'Add machine', url: '/resources/machines', clicks: [click('@machines.list.add')], dialog: '@machines.add.title' },
  { id: 'add-agent', label: 'Add agent profile', url: '/resources/agents', clicks: [click('@agents.list.add')], dialog: '@agents.add.title' },
  { id: 'add-host', label: 'Add host', url: '/resources/hosts', clicks: [click('@hosts.list.add')], dialog: '@hosts.add.title' },
  { id: 'new-template', label: 'New template', url: '/resources/templates', clicks: [click('@templates.list.add')], dialog: '@templates.new.title' },
  { id: 'make-environment', label: 'Make an environment', url: '/resources/environments', clicks: [click('@environments.list.add'), { role: 'menuitem', name: '@environments.list.makeFromTemplate' }], dialog: '@environments.make.title' },
  { id: 'new-token', label: 'New personal token', url: '/settings/tokens', clicks: [click('@tokens.new.button')], dialog: '@tokens.new.title' },
  { id: 'keyboard-shortcuts', label: 'Keyboard shortcuts', url: '/settings/interface', clicks: [click('@shell.keyChoice.single.editor')], dialog: '@shell.shortcuts.title' },
];

export function captureManifest(config: Pick<CaptureConfig, 'targets' | 'routes' | 'replacements' | 'dialogs' | 'states'>): CaptureState[] {
  const prefixes = Object.entries(config.targets).map(([key, path]) => {
    const seed = Object.hasOwn(TARGET_SEEDS, key) ? TARGET_SEEDS[key] : undefined;
    if (seed === undefined) throw new Error('An unknown capture target was supplied; see TARGET_SEEDS.');
    return { seed, path: path.replace(/\/$/, '') };
  }).sort((left, right) => right.seed.length - left.seed.length);
  const rows: CaptureState[] = [
    ...SIGNED_IN_ROUTES.map((route, index): CaptureState => ({ id: `route-${String(index + 1).padStart(3, '0')}`, label: route.url, pattern: route.pattern,
      url: route.url, signedIn: true, kind: 'route', note: route.note })),
    ...SIGNED_OUT_ROUTES.map((route, index): CaptureState => ({ id: `public-${String(index + 1).padStart(2, '0')}`, label: route.url, pattern: route.pattern,
      url: route.url, signedIn: false, kind: 'route', note: route.scenario === undefined ? route.note : 'Existing world: /setup may refuse setup or redirect. This tool never resets it to fresh.' })),
  ];
  const queries: [string, string][] = [
    ['Org members', '/org/acme?tab=members'], ['Org activity', '/org/acme?tab=activity'],
    ['Project activity', `${project}?view=activity`], ['Project done', `${project}?status=done`], ['Project archived', `${project}?archived=true`],
    ...['threads', 'environments', 'documents', 'agents', 'activity'].map((tab): [string, string] => [`Topic ${tab}`, `${topic}?tab=${tab}`]),
    ['Thread catch-up dialog', `${thread}?catchup=true`], ['Thread turn permalink', `${thread}?turn=1`],
    ['Unread inbox', '/inbox?filter=unread'], ['Mentions', '/inbox?filter=mentions'],
    ['My requests', '/approvals?tab=requested'], ['Decided requests', '/approvals?tab=history'], ['All approvals', '/approvals?all=true'],
    ['Search threads', '/search?q=thread'], ['Everything I participate in', '/everything?participating=true'],
    ...['registered', 'sessions', 'policy', 'certificate', 'access'].map((tab): [string, string] => [`Machine ${tab}`, `${machine}?tab=${tab}`]),
    ...['provider-readme', 'provider-facts', 'provider-models', 'provider-resets', 'provider-credential'].map((article): [string, string] => [`Help ${article}`, `/help/${article}`]),
  ];
  rows.push(...queries.map(([label, url], index): CaptureState => ({ id: `query-${String(index + 1).padStart(2, '0')}`, label,
    pattern: url.split('?')[0] ?? url, url, signedIn: true, kind: 'query', ...(url.endsWith('?catchup=true') ? { dialog: '*' } : {}) })));
  rows.push(...THREAD_CONTAINERS.map((container): CaptureState => ({ id: `container-${container}`, label: `Thread / ${container}`, pattern: '/thread/$threadId',
    url: thread, signedIn: true, kind: 'container', container })));
  rows.push(...THREAD_VIEWS.map((view): CaptureState => ({ id: `view-${view}`, label: `Thread / ${view}`, pattern: '/thread/$threadId',
    url: thread, signedIn: true, kind: 'view', view })));
  rows.push(...DIALOGS.map((row): CaptureState => ({ ...row, signedIn: true, kind: 'dialog', pattern: row.url.split('?')[0] ?? row.url })));
  rows.push(...config.dialogs.map((row): CaptureState => ({ ...row, id: `extra-${row.id}`, signedIn: true, kind: 'dialog', pattern: row.url.split('?')[0] ?? row.url })));
  rows.push(...config.states.map((row): CaptureState => ({ ...row, id: `extra-url-${row.id}`, signedIn: true, kind: 'query', pattern: row.url.split('?')[0] ?? row.url })));
  const ids = rows.map((row) => row.id);
  if (new Set(ids).size !== ids.length) throw new Error('Capture state ids must be unique.');
  return rows.map((row) => {
    // Exact per-URL overrides win. Otherwise a copied real target path carries every tab and child section.
    if (Object.hasOwn(config.routes, row.url)) return { ...row, url: mapRoute(row.url, config) };
    const parsed = new URL(row.url, 'http://capture.invalid');
    const prefix = prefixes.find(({ seed }) => parsed.pathname === seed || parsed.pathname.startsWith(`${seed}/`));
    if (prefix !== undefined) parsed.pathname = prefix.path + parsed.pathname.slice(prefix.seed.length);
    return { ...row, url: mapRoute(parsed.pathname + parsed.search + parsed.hash, config) };
  });
}
