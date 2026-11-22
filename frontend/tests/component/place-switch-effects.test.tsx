/** A place's switches of mounts, MCP servers, templates and skills say, as an environment's rows do, when a change lands and
 * what it reaches: the environments that still take the place's value with their threads, or its threads; switching a mount
 * off at a place says that running environments keep it until their next attachment, with what each place reaches. */
import { screen, within } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { PAGE_SIZE } from '@/api/nodes';
import { reachSnapshot, usePlaceSwitchEffect } from '@/features/settings/EffectLine';
import { SettingRow } from '@/features/settings/SettingRow';
import type { KeyDef } from '@/features/settings/settingsModel';
import { MountEffects } from '@/features/storage/Impact';
import type { ResolvedSetting, ResourceFacts, SettingReach } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

interface ReadOptions { params: { path: { id?: string; key: string }; query: { limit: number } } }
const calls = vi.hoisted(() => ({ read: vi.fn<(path: string, options: ReadOptions) => Promise<{ data: SettingReach; response: Response }>>() }));
vi.mock('@/api/client', async (load) => ({ ...await load<object>(), api: { GET: calls.read } }));
vi.mock('@/features/chat/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useOpenPath: () => vi.fn() }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));

const target = { kind: 'node', id: 'login-topic' } as const;
const place = { name: 'Login', kind: 'topic' as const, owner: null, access: { permissions: ['topic.view'] } };
const definition = (key: string, title: string): KeyDef => ({ key, title, description: `${title} of the threads here.`, group: 'environment',
  class: 'parameter', scopes: ['org', 'project', 'topic', 'environment'], merge: 'map', schema: { type: 'object' }, default: {}, example: {},
  text_key: null, keywords: [] });
const setting = (key: string): ResolvedSetting => ({ key, value: {}, own: null, writable: true, locked_by: null, source: { scope: 'default', node: null }, parts: [] });
const reach = (applies: Partial<SettingReach['applies']>): { data: SettingReach; response: Response } => ({
  data: { applies: { orgs: 0, projects: 0, topics: 0, threads: 0, ...applies } as SettingReach['applies'], overrides: { items: [], next_cursor: null } },
  response: new Response(null, { status: 200 }),
});

function Row({ def }: { def: KeyDef }) {
  const effectFor = usePlaceSwitchEffect(target);
  const value = setting(def.key);
  return <ul><SettingRow def={def} setting={value} target={target} place={place} effectNote={effectFor(def.key, value)} /></ul>;
}
/** The effect line, whose words stand in more than one element. */
const line = (words: string) => (_content: string, element: Element | null) => element?.tagName === 'SPAN' && element.classList.contains('text-xs') && element.textContent === words;
function row(title: string): HTMLElement {
  const found = screen.getByText(title, { selector: '[data-setting-row] *' }).closest<HTMLElement>('[data-setting-row]');
  if (found === null) throw new Error(`no row for ${title}`);
  return found;
}

beforeEach(async () => { await setLocale('en'); vi.clearAllMocks(); });

it.each(['en', 'zh-CN'] as const)('a topic\'s Mounts say when a change lands there and the environments and threads it reaches (%s)', async (locale) => {
  await setLocale(locale);
  calls.read.mockResolvedValue(reach({ threads: 9, environments: 2, environment_threads: 3 }));
  show(<Row def={definition('env.mounts', 'Mounts')} />);
  const mounts = row('Mounts');
  const words = `${i18n.t('runtimeChanges.takesEffect')}: ${i18n.t('unitConfiguration.effects.placeMounts')} · ${i18n.t('unitConfiguration.effects.environmentsWithThreads', {
    environments: i18n.t('unitConfiguration.effects.environments', { count: 2, number: '2' }), threads: i18n.t('unitConfiguration.effects.threads', { count: 3, number: '3' }) })}`;
  expect(await within(mounts).findByText(line(words))).toBeVisible();
  // The same read as the row's own Where it applies.
  expect(calls.read).toHaveBeenCalledExactlyOnceWith('/nodes/{id}/settings/{key}/reach', expect.objectContaining({ params: { path: { id: target.id, key: 'env.mounts' }, query: { limit: PAGE_SIZE } } }));
  expect(reachSnapshot(setting('env.mounts'))).toBe(JSON.stringify([{}, null, null, { scope: 'default', node: null }, []]));
});

it('Skills say when a change lands and the threads they reach, and a topic no environment takes says so', async () => {
  calls.read.mockImplementation((_path, { params }) => Promise.resolve(params.path.key === 'agents.skills'
    ? reach({ threads: 4, environments: 0, environment_threads: 0 }) : reach({ threads: 4, environments: 0, environment_threads: 0 })));
  show(<><Row def={definition('agents.skills', 'Skills')} /><Row def={definition('mcp.enabled', 'MCP servers')} /><Row def={definition('env.template_id', 'Template')} /></>);
  expect(await within(row('Skills')).findByText(line('Takes effect: At the next session · 4 threads'))).toBeVisible();
  expect(await within(row('MCP servers')).findByText(line('Takes effect: Before the next turn · No environment here takes it'))).toBeVisible();
  expect(await within(row('Template')).findByText(line('Takes effect: After a reviewed update · No environment here takes it'))).toBeVisible();
});

it('a server that counts no environments yet leaves the line at its apply time, and a key that switches nothing has no line', async () => {
  calls.read.mockResolvedValue(reach({ threads: 4 }));
  show(<><Row def={definition('env.mounts', 'Mounts')} /><Row def={definition('env.idle_stop_minutes', 'Idle stop')} /></>);
  expect(await within(row('Mounts')).findByText(line(`Takes effect: ${i18n.t('unitConfiguration.effects.placeMounts')}`))).toBeVisible();
  expect(within(row('Idle stop')).queryByText(/Takes effect/)).not.toBeInTheDocument();
  expect(calls.read).toHaveBeenCalledOnce();
});

it('switching a mount off at a place says running environments keep it until their next attachment, and what each place reaches', async () => {
  calls.read.mockImplementation((_path, { params }) => Promise.resolve(params.path.id === 'login-topic'
    ? reach({ environments: 1, environment_threads: 2 }) : reach({ environments: 0, environment_threads: 0 })));
  const facts = { dependents: [{ relation: 'explicit_giving', count: 2, hidden: 0, visible: [
    { name: 'Login', object: { kind: 'topic', id: 'login-topic' }, relation: 'explicit_giving', until: null },
    { name: 'Web', object: { kind: 'project', id: 'web-project' }, relation: 'explicit_giving', until: null },
  ] }] } as unknown as ResourceFacts;
  show(<MountEffects facts={facts} />);
  expect(screen.getByText('2 places switch off now')).toBeVisible();
  expect(screen.getByText('Running environments keep it until their next attachment')).toBeVisible();
  expect(await screen.findByText('· 1 environment with 2 threads')).toBeVisible();
  expect(await screen.findByText('· No environment here takes it')).toBeVisible();
  expect(calls.read).toHaveBeenCalledTimes(2);
  expect(calls.read).toHaveBeenCalledWith('/nodes/{id}/settings/{key}/reach', expect.objectContaining({ params: { path: { id: 'web-project', key: 'env.mounts' }, query: { limit: PAGE_SIZE } } }));
});
