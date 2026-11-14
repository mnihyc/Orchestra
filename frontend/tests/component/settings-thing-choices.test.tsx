/** A setting that names a thing that exists is chosen by name, never typed as an identifier, and writes the same shape it always did. */
import { infiniteQueryOptions } from '@tanstack/react-query';
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import { clearSetting, putSetting } from '@/api/settings';
import { SettingRow } from '@/features/settings/SettingRow';
import { EmbeddingsChoice } from '@/features/settings/EmbeddingsChoice';
import { DocumentSetChoice, MountSettingChoice, ThingSettingChoice } from '@/features/settings/ThingChoice';
import { shapeOf, withGroup } from '@/features/settings/settingsModel';
import type { Agent, Environment, ResolvedSetting } from '@/generated/api-types';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { i18n } from '@/i18n';
import { detailAccess, detailAgent, detailEnvironment, detailMount } from './resource-detail-fixtures';
import { show } from './support';

const server = vi.hoisted(() => ({
  environments: [] as unknown[], agents: [] as unknown[], mounts: [] as unknown[], models: [] as unknown[], found: [] as unknown[],
  missing: new Set<string>(), next: [] as unknown[], failNext: false,
}));
const notFound = () => new ApiError({ type: 'orch:error:not_found', code: 'not_found', title: 'Not found', status: 404, detail: 'There is no such thing.', request_id: 'thing-missing' }, null);
const rows = (list: unknown[]) => Object.assign([...list], { privateRows: [] });
vi.mock('@/api/environments', async (load) => ({ ...await load<object>(),
  environmentsQuery: () => ({ queryKey: ['thing-environments'], queryFn: () => Promise.resolve(rows(server.environments)) }),
  environmentQuery: (id: string) => ({ queryKey: ['thing-environment', id], queryFn: () => server.missing.has(id) ? Promise.reject(notFound()) : Promise.resolve(server.environments.find((row) => (row as { id: string }).id === id)) }),
}));
vi.mock('@/api/resources', async (load) => ({ ...await load<object>(),
  agentsQuery: () => ({ queryKey: ['thing-agents'], queryFn: () => Promise.resolve(rows(server.agents)) }),
  agentQuery: (id: string) => ({ queryKey: ['thing-agent', id], queryFn: () => server.missing.has(id) ? Promise.reject(notFound()) : Promise.resolve(server.agents.find((row) => (row as { id: string }).id === id)) }),
  providerModelsQuery: (id: string) => ({ queryKey: ['thing-models', id], queryFn: () => Promise.resolve(server.models) }),
}));
vi.mock('@/api/resource-tools', async (load) => ({ ...await load<object>(), mountsQuery: { queryKey: ['thing-mounts'], queryFn: () => Promise.resolve(rows(server.mounts)) } }));
vi.mock('@/api/documents', async (load) => ({ ...await load<object>(),
  documentQuery: (id: string) => ({ queryKey: ['thing-document', id], queryFn: () => Promise.resolve({ id, title: id === 'doc-1' ? 'Release notes' : 'Other', deleted: false }) }),
}));
vi.mock('@/api/settings', async (load) => ({ ...await load<object>(), clearSetting: vi.fn(), putSetting: vi.fn() }));
vi.mock('@/api/everything', async (load) => ({ ...await load<object>(),
  everythingQuery: (_reader: string, _parent: string | null, filter: { q: string }) => infiniteQueryOptions({ queryKey: ['thing-search', filter.q], initialPageParam: null as string | null,
    queryFn: ({ pageParam }) => pageParam !== null && server.failNext ? Promise.reject(notFound()) : Promise.resolve({ items: filter.q === '' ? [] : pageParam === null ? server.found : server.next, next_cursor: pageParam === null && server.next.length > 0 ? 'next' : null }), getNextPageParam: (page) => page.next_cursor ?? undefined }),
}));
// The provider list is the product's own picker, proved elsewhere; here a choice is one press.
vi.mock('@/features/settings/ProviderChoice', async (load) => ({ ...await load<object>(),
  ProviderDefaultChoice: ({ onChange }: { onChange: (next: unknown) => void }) => <button type="button" onClick={() => onChange('provider-2')}>Choose provider</button>,
}));

const allowed = { ...detailAccess, permissions: [...detailAccess.permissions, 'agent.use', 'mount.use'] };
const environment = (id: string, name: string, manage: boolean): Environment => ({ ...detailEnvironment, id, name, access: { ...detailAccess, permissions: manage ? detailAccess.permissions : detailAccess.permissions.filter((one) => one !== 'environment.manage') } });
const agent = (id: string, name: string, use: boolean): Agent => ({ ...detailAgent, id, name, access: use ? allowed : detailAccess });
beforeEach(() => { vi.clearAllMocks(); server.next = []; server.failNext = false; server.environments = []; server.agents = []; server.mounts = []; server.models = []; server.found = []; server.missing = new Set(); });

function Single({ settingKey, initial, changed }: { settingKey: 'env.default_id' | 'agent.default_agent_id'; initial: unknown; changed: (value: unknown) => void }) {
  const [value, setValue] = useState<unknown>(initial);
  return <ThingSettingChoice settingKey={settingKey} shape={{ type: 'id' }} label="The choice" value={value} onChange={(next) => { setValue(next); changed(next); }} />;
}

it('chooses the default environment by name, greys one that is not the person\'s to manage with its reason, and writes only the identifier', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  server.environments = [environment('env-1', 'Research workspace', true), environment('env-2', 'Locked workspace', false)];
  show(<Single settingKey="env.default_id" initial="" changed={changed} />);
  await user.click(screen.getByRole('combobox', { name: 'The choice' }));
  const refused = await screen.findByRole('option', { name: /Locked workspace/ });
  expect(refused).toHaveTextContent(i18n.t('settingsEditors.thing.needsManage'));
  await user.click(refused);
  expect(changed).not.toHaveBeenCalled();
  await user.click(screen.getByRole('option', { name: /Research workspace/ }));
  expect(changed).toHaveBeenCalledExactlyOnceWith('env-1');
  await waitFor(() => expect(screen.getByRole('combobox', { name: 'The choice' })).toHaveValue('Research workspace'));
});

it('chooses the default agent profile by name and keeps one the person may not use out of reach', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  server.agents = [agent('agent-1', 'Research profile', true), agent('agent-2', 'Private profile', false)];
  show(<Single settingKey="agent.default_agent_id" initial="" changed={changed} />);
  await user.click(screen.getByRole('combobox', { name: 'The choice' }));
  const refused = await screen.findByRole('option', { name: /Private profile/ });
  expect(refused).toHaveTextContent(i18n.t('settingsEditors.thing.needsUse', { kind: i18n.t('settingsEditors.thing.kinds.agent') }));
  await user.click(refused);
  expect(changed).not.toHaveBeenCalled();
  await user.click(screen.getByRole('option', { name: /Research profile/ }));
  expect(changed).toHaveBeenCalledExactlyOnceWith('agent-1');
});

it.each(['env.default_id', 'agent.default_agent_id'] as const)('clears a deleted default at %s through the revisioned setting writer', async (key) => {
  const user = userEvent.setup();
  server.missing = new Set(['thing-gone']);
  const def = SETTINGS_REGISTRY.find((entry) => entry.key === key);
  if (def === undefined) throw new Error('The registry must declare the default choice.');
  const target = { kind: 'node', id: 'topic' } as const;
  const setting: ResolvedSetting = { key, value: 'thing-gone', own: { value: 'thing-gone', revision: 7, locked: false }, source: { scope: 'topic', node: null },
    writable: true, locked_by: null, parts: [] };
  vi.mocked(clearSetting).mockResolvedValue({ ...setting, value: null, own: null, source: { scope: 'default', node: null } });
  const view = show(<ul><SettingRow def={withGroup(def)} setting={setting} target={target} place={{ name: 'Topic', kind: 'topic', owner: null, access: null }} /></ul>);
  expect(await screen.findByText(i18n.t('settingsEditors.thing.goneAbout', { kind: key === 'env.default_id' ? 'environment' : 'agent profile' }))).toBeVisible();
  expect(view.container).not.toHaveTextContent('thing-gone');
  await user.click(screen.getByRole('button', { name: i18n.t('settingsEditors.thing.clear') }));
  await waitFor(() => expect(clearSetting).toHaveBeenCalledExactlyOnceWith(target, key, 7));
  expect(putSetting).not.toHaveBeenCalled();
});

it('chooses mounts by name with how each is attached, and writes the same map of identifiers', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  server.mounts = [{ ...detailMount, access: allowed }];
  function Mounts() {
    const [value, setValue] = useState<unknown>({});
    return <MountSettingChoice shape={{ type: 'json' }} label="Mounts" value={value} onChange={(next) => { setValue(next); changed(next); }} />;
  }
  show(<Mounts />);
  await user.click(screen.getByRole('combobox', { name: i18n.t('settingsEditors.thing.addMount') }));
  await user.click(await screen.findByRole('option', { name: /research-data/ }));
  expect(changed).toHaveBeenLastCalledWith({ [detailMount.id]: true });
  const how = await screen.findByRole('combobox', { name: i18n.t('settingsEditors.thing.attachedAs', { name: 'research-data' }) });
  await user.click(how);
  await user.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.mountMode.read_only.name') }));
  expect(changed).toHaveBeenLastCalledWith({ [detailMount.id]: 'read_only' });
  await user.click(screen.getByRole('button', { name: i18n.t('settingsEditors.thing.remove', { name: 'research-data' }) }));
  expect(changed).toHaveBeenLastCalledWith({});
});

it('adds starting documents by searching their names and removes them by name, writing the list of identifiers', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  server.found = [{ kind: 'document', id: 'doc-1', name: 'Release notes' }];
  function Documents() {
    const [value, setValue] = useState<unknown>([]);
    return <DocumentSetChoice shape={{ type: 'json' }} label="Documents" value={value} onChange={(next) => { setValue(next); changed(next); }} />;
  }
  show(<Documents />);
  await user.type(screen.getByRole('combobox', { name: i18n.t('settingsEditors.thing.addDocument') }), 'Rel');
  await user.click(await screen.findByRole('option', { name: 'Release notes' }));
  expect(changed).toHaveBeenLastCalledWith(['doc-1']);
  const list = await screen.findByRole('list', { name: 'Documents' });
  expect(await within(list).findByText('Release notes')).toBeVisible();
  expect(list).not.toHaveTextContent('doc-1');
  await user.click(within(list).getByRole('button', { name: i18n.t('settingsEditors.thing.remove', { name: 'Release notes' }) }));
  await waitFor(() => expect(changed).toHaveBeenLastCalledWith([]));
});

it('chooses the embeddings model among the provider\'s enabled models, and the model follows the provider', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  const registered = SETTINGS_REGISTRY.find((def) => def.key === 'search.semantic.embeddings');
  if (registered === undefined) throw new Error('The registry must declare the embeddings.');
  server.models = [{ id: 'm1', model_name: 'embed-small', display_name: 'Small embedder', enabled: true }, { id: 'm2', model_name: 'embed-large', display_name: 'Large embedder', enabled: false }];
  show(<EmbeddingsChoice shape={shapeOf(registered.schema)} label="Embeddings" onChange={changed}
    value={{ provider_id: 'provider-1', model_name: '', dimensions: 256 }} />);
  await user.click(screen.getByRole('combobox', { name: i18n.t('admin.semantic.fields.model') }));
  expect(await screen.findByRole('option', { name: 'Small embedder' })).toBeVisible();
  expect(screen.queryByRole('option', { name: 'Large embedder' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('option', { name: 'Small embedder' }));
  expect(changed).toHaveBeenLastCalledWith({ provider_id: 'provider-1', model_name: 'embed-small', dimensions: 256 });
  await user.click(screen.getByRole('button', { name: 'Choose provider' }));
  expect(changed).toHaveBeenLastCalledWith({ provider_id: 'provider-2', model_name: '', dimensions: 256 });
});


it('reaches later starting documents even after every name on the first search page has been selected', async () => {
  const user = userEvent.setup(), changed = vi.fn();
  const initial = Array.from({ length: 50 }, (_, index) => `doc-${index}`);
  server.found = initial.map((id) => ({ kind: 'document', id, name: 'Release notes' }));
  server.next = [{ kind: 'document', id: 'later-document', name: 'Release notes' }];
  function Documents() {
    const [value, setValue] = useState<unknown>(initial);
    return <DocumentSetChoice shape={{ type: 'json' }} label="Documents" value={value} onChange={(next) => { setValue(next); changed(next); }} />;
  }
  show(<Documents />);
  await user.type(screen.getByRole('combobox', { name: i18n.t('settingsEditors.thing.addDocument') }), 'Release');
  server.failNext = true;
  await user.click(await screen.findByRole('button', { name: i18n.t('ui.pagination.more') }));
  await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
  server.failNext = false;
  await user.click(await screen.findByRole('button', { name: i18n.t('common.tryAgain') }));
  await user.click(await screen.findByRole('option', { name: 'Release notes' }));
  expect(changed).toHaveBeenLastCalledWith([...initial, 'later-document']);
});
