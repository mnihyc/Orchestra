/** Resource confirmations wait for complete effects, retain emergency-stop intent and keep switching separate. */
import { QueryClient } from '@tanstack/react-query';
import tokenDensity from '../../../crates/control/engine/data/token-density.json' with {type:'json'};
import type * as ResourceApi from '@/api/resources';
import { cleanup, screen, waitFor, act } from '@testing-library/react';
import type * as ResourceTools from '@/api/resource-tools';
import { ApiError } from '@/api/client';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DeleteProviderDialog, DisableProviderDialog } from '@/features/resources/providers/DangerZone';
import { EditMount } from '@/features/resources/mounts/Edit';
import { ModelLifecycle } from '@/features/resources/providers/ModelLifecycle';
import { DeleteMcpDialog } from '@/features/resources/mcp/Preset';
import { ImpactCounts } from '@/features/storage/Impact';
import type { FactTarget, ProviderModel, ResourceImpact } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';
import { ADA } from '../doubles/chat-rows';
import { detailMcp, detailMount } from './resource-detail-fixtures';

const calls = vi.hoisted(() => ({ read: vi.fn<(target: FactTarget) => Promise<ResourceImpact>>(), disable: vi.fn(), remove: vi.fn(), removeMcp: vi.fn(), updateMount: vi.fn<typeof ResourceTools.updateMount>(), updateModel:vi.fn(),removeModel:vi.fn(), switch: vi.fn() }));
vi.mock('@/api/resource-impact', () => ({ resourceImpactQuery: (target: FactTarget) => ({ queryKey: ['pause-dialog', target], queryFn: () => calls.read(target), retry: false }) }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourceApi>(), disableProvider: calls.disable, deleteProvider: calls.remove,updateModel:calls.updateModel,deleteModel:calls.removeModel }));
vi.mock('@/api/resource-tools', async (original) => ({ ...await original<typeof ResourceTools>(), deleteMcp: calls.removeMcp, updateMount: calls.updateMount }));
vi.mock('@/features/change/target', async (loadOriginal) => ({ ...await loadOriginal<object>(), openChange: calls.switch }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));

const provider = planProvider();
const model:ProviderModel={id:'model',model_name:'route-model',display_name:'Research model',enabled:true,source:'manual',groups:[],parameters:{},token_estimate:{...tokenDensity.unmeasured,measured:false}};
const thread = { object: { kind: 'thread' as const, id: '00000000-0000-4000-8000-000000000101' }, name: 'Repair redirects', relation: 'binding' as const, until: null };
function impact(): ResourceImpact {
  const empty = { count: 0, threads: 0, visible: [], hidden: 0, since: null };
  return { target: { object: { kind: 'provider', id: provider.id }, model_id: null }, name: provider.name, stopping: false, applications: [], application_complete: true,
    activity: { running: { count: 2, threads: 2, visible: [thread], hidden: 1, since: null }, waiting: empty, stopped: empty },
    facts: { subject: { kind: 'resource', object: { kind: 'provider', id: provider.id } }, as_of: new Date().toISOString(), dependents_complete: true, uses: null,
      dependents: [{ relation: 'binding', count: 5, visible: [thread], hidden: 4, until: null }] },
  };
}

for (const locale of ['en', 'zh-CN'] as const) for (const notify of [false, true]) {
  it(`shows current MCP dependents before confirming an explicit ${notify ? 'selected' : 'cleared'} notice (${locale})`, async () => {
    await setLocale(locale);
    const target = { object: { kind: 'mcp_server' as const, id: detailMcp.id }, model_id: null };
    const current = { ...impact(), target, name: detailMcp.name, notification: { preview_id: "reviewed", people:2, audience: { people: [ADA], hidden: 1 }, default_on: true } };
    current.facts = { ...current.facts, subject: { kind: 'resource', object: target.object }, dependents: [
      { relation: 'attachment', count: 3, hidden: 2, until: null, visible: [{ ...thread, relation: 'attachment' }] },
    ] };
    calls.read.mockResolvedValue(current);
    const user = userEvent.setup();
    const removed = vi.fn();
    show(<DeleteMcpDialog server={{ ...detailMcp, imported: null }} onClose={vi.fn()} onRemoved={removed} />);
    const choice = await screen.findByRole('checkbox', { name: i18n.t('programVersions.notify', { count: 2, number: '2' }) });
    expect(choice).toBeChecked();
    expect(calls.read).toHaveBeenCalledWith(target);
    expect(screen.getByText(i18n.t('library.given', { number: '3', unit: i18n.t('library.units.attachments', { count: 3 }) }))).toBeVisible();
    if (!notify) await user.click(choice);
    await user.click(screen.getByRole('button', { name: i18n.t('resourceDetails.remove') }));
    await waitFor(() => expect(calls.removeMcp.mock.calls).toEqual([notify ? [detailMcp.id,{notify:true,notification_review:'reviewed'}] : [detailMcp.id]]));
    await waitFor(() => expect(removed).toHaveBeenCalledOnce());
  });
}
beforeEach(() => { calls.updateMount.mockResolvedValue(detailMount); calls.read.mockResolvedValue(impact()); calls.disable.mockResolvedValue(provider); calls.remove.mockResolvedValue(undefined); calls.updateModel.mockResolvedValue(model);calls.removeModel.mockResolvedValue(undefined); calls.updateMount.mockResolvedValue(detailMount); });
afterEach(async () => { cleanup(); vi.resetAllMocks(); await setLocale('en'); });

it('shows the running work without holding up an emergency stop and sends its note with an explicit silent choice', async () => {
  const user = userEvent.setup();
  let ready: ((value: ResourceImpact) => void) | undefined;
  calls.read.mockImplementation(() => new Promise<ResourceImpact>((resolve) => { ready = resolve; }));
  show(<DisableProviderDialog provider={provider} open onClose={vi.fn()} />);
  const confirm = screen.getByRole('button', { name: i18n.t('providers.danger.disableConfirm', { name: provider.name }) });
  expect(confirm).toBeEnabled();
  expect(calls.disable).not.toHaveBeenCalled();
  await waitFor(() => expect(ready).toBeTypeOf('function'));
  ready?.(impact());
  expect(await screen.findByText('2 turns stop now')).toBeVisible();
  await user.click(screen.getByRole('button',{name:i18n.t('resourceImpact.showUses')}));
  expect(screen.getByText('5 threads stay bound')).toBeVisible();
  expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('resources.act.switchThreads') })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('resourceImpact.switchFirst') })).not.toBeInTheDocument();
  await user.type(screen.getByRole('textbox', { name: new RegExp(i18n.t('resourceImpact.note')) }), '  Key rotation  ');
  await user.click(confirm);
  await waitFor(() => expect(calls.disable).toHaveBeenCalledExactlyOnceWith(provider.id, 'Key rotation'));
});

it('keeps the stop available and warns about running turns when their count cannot be read', async () => {
  calls.read.mockRejectedValue(new Error('The current dependencies could not be read.'));
  show(<DisableProviderDialog provider={provider} open onClose={vi.fn()} />);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button',{name:i18n.t('resourceImpact.showUses')}));
  await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
  expect(screen.getByRole('button', { name: i18n.t('providers.danger.disableConfirm', { name: provider.name }) })).toBeEnabled();
  expect(screen.getByText(i18n.t('resourceImpact.stopTip'))).toBeVisible();
  expect(screen.queryByText('0 turns stop now')).not.toBeInTheDocument();
});

it('deleting requires the current exact name and offers thread switching as an independent act', async () => {
  const user = userEvent.setup();
  const close = vi.fn();
  show(<DeleteProviderDialog provider={provider} open onClose={close} />);
  await user.click(await screen.findByRole('button',{name:i18n.t('resourceImpact.showUses')}));
  expect(await screen.findByText('5 threads stay bound')).toBeVisible();
  const confirm = screen.getByRole('button', { name: i18n.t('providers.danger.deleteConfirm', { name: provider.name }) });
  expect(confirm).toBeDisabled();
  const name = screen.getByRole('textbox', { name: i18n.t('resourceImpact.typeName', { name: provider.name }) });
  await user.type(name, `${provider.name} extra`);
  expect(confirm).toBeDisabled();
  expect(calls.remove).not.toHaveBeenCalled();
  await user.clear(name); await user.type(name, provider.name);
  expect(confirm).toBeEnabled();
  await user.click(screen.getByRole('button', { name: i18n.t('resourceImpact.switchFirst') }));
  expect(close).toHaveBeenCalledOnce();
  expect(calls.switch).toHaveBeenCalledExactlyOnceWith({ kind: 'provider', id: provider.id, name: provider.name }, { kind: 'switch' });
  expect(calls.remove).not.toHaveBeenCalled();
});

it('a model stop reads only its exact provider membership and keeps switching out of Disable',async()=>{
  const user=userEvent.setup({delay:null});
  calls.read.mockResolvedValue({...impact(),name:model.display_name,target:{object:{kind:'provider',id:provider.id},model_id:model.id}});
  show(<ModelLifecycle provider={provider} model={model} kind="disable" onClose={vi.fn()} />);
  await screen.findByText('2 turns stop now');
  expect(calls.read).toHaveBeenCalledWith({object:{kind:'provider',id:provider.id},model_id:model.id});
  expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  expect(screen.queryByRole('button',{name:i18n.t('resourceImpact.switchFirst')})).not.toBeInTheDocument();
  await user.type(screen.getByRole('textbox',{name:new RegExp(i18n.t('resourceImpact.note'))}),'  Model pause  ');
  await user.click(screen.getByRole('button',{name:i18n.t('providers.danger.disableConfirm',{name:model.display_name})}));
  await waitFor(()=>expect(calls.updateModel).toHaveBeenCalledExactlyOnceWith(provider.id,model.id,{enabled:false},'Model pause',undefined,false));
});

it('a model deletion requires its exact current name before ending its membership',async()=>{
  const user=userEvent.setup({delay:null});
  calls.read.mockResolvedValue({...impact(),name:model.display_name,target:{object:{kind:'provider',id:provider.id},model_id:model.id}});
  show(<ModelLifecycle provider={provider} model={model} kind="delete" onClose={vi.fn()} />);
  await user.click(await screen.findByRole('button',{name:i18n.t('resourceImpact.showUses')}));
  await screen.findByText('5 threads stay bound');
  const submit=screen.getByRole('button',{name:i18n.t('providers.danger.deleteConfirm',{name:model.display_name})});
  expect(submit).toBeDisabled();
  expect(calls.removeModel).not.toHaveBeenCalled();
  await user.type(screen.getByRole('textbox',{name:i18n.t('resourceImpact.typeName',{name:model.display_name})}),model.display_name);
  await user.click(submit);
  await waitFor(()=>expect(calls.removeModel).toHaveBeenCalledExactlyOnceWith(provider.id,model.id,false));
});

for (const locale of ['en', 'zh-CN'] as const) for (const kind of ['disable', 'delete'] as const) for (const notify of [false, true]) {
  it(`confirms the ${notify ? 'selected' : 'cleared'} ${kind} notification choice explicitly (${locale})`, async () => {
    await setLocale(locale);
    const current = { ...impact(), notification: { preview_id: "reviewed", people:3, audience: { people: [ADA], hidden: 2 }, default_on: true } };
    calls.read.mockResolvedValue(current);
    const user = userEvent.setup();
    show(kind === 'disable' ? <DisableProviderDialog provider={provider} open onClose={vi.fn()} /> : <DeleteProviderDialog provider={provider} open onClose={vi.fn()} />);
    const name = i18n.t('programVersions.notify', { count: 3, number: '3' });
    const choice = await screen.findByRole('checkbox', { name });
    expect(choice).toBeChecked();
    if (!notify) await user.click(choice);
    if (notify) expect(choice).toBeChecked(); else expect(choice).not.toBeChecked();
    if (kind === 'delete') await user.type(screen.getByRole('textbox', { name: i18n.t('resourceImpact.typeName', { name: provider.name }) }), provider.name);
    await user.click(screen.getByRole('button', { name: i18n.t(kind === 'delete' ? 'providers.danger.deleteConfirm' : 'providers.danger.disableConfirm', { name: provider.name }) }));
    await waitFor(() => expect(kind === 'disable' ? calls.disable : calls.remove).toHaveBeenCalled());
    const review = {notify:true, notification_review:'reviewed'};
    const expected = notify ? (kind === 'disable' ? [provider.id, undefined, review] : [provider.id, review]) : [provider.id];
    expect((kind === 'disable' ? calls.disable : calls.remove).mock.calls).toEqual([expected]);
  });
}


for (const locale of ['en', 'zh-CN'] as const) for (const kind of ['disable', 'delete'] as const) for (const notify of [false, true]) {
  it(`keeps the exact model lifecycle notice choice explicit when ${kind} is ${notify ? 'selected' : 'cleared'} (${locale})`, async () => {
    await setLocale(locale);
    calls.read.mockResolvedValue({ ...impact(), name: model.display_name,
      target: { object: { kind: 'provider', id: provider.id }, model_id: model.id },
      notification: { preview_id: "reviewed", people:3, audience: { people: [ADA], hidden: 2 }, default_on: true },
    });
    const user = userEvent.setup();
    show(<ModelLifecycle provider={provider} model={model} kind={kind} onClose={vi.fn()} />);
    const choice = await screen.findByRole('checkbox', { name: i18n.t('programVersions.notify', { count: 3, number: '3' }) });
    expect(choice).toBeChecked();
    if (!notify) await user.click(choice);
    if (kind === 'delete') await user.type(screen.getByRole('textbox', { name: i18n.t('resourceImpact.typeName', { name: model.display_name }) }), model.display_name);
    await user.click(screen.getByRole('button', { name: i18n.t(kind === 'delete' ? 'providers.danger.deleteConfirm' : 'providers.danger.disableConfirm', { name: model.display_name }) }));
    if (kind === 'delete') await waitFor(() => expect(calls.removeModel).toHaveBeenCalledExactlyOnceWith(provider.id, model.id, notify));
    else await waitFor(() => expect(calls.updateModel).toHaveBeenCalledExactlyOnceWith(provider.id, model.id, { enabled: false }, undefined, undefined, notify));
  });
}

it.each(['en', 'zh-CN'] as const)('provider operations keep notification off until this operation is chosen (%s)', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup({delay:null});
  calls.read.mockResolvedValue(Object.assign(impact(), {notification: {preview_id: 'reviewed-people', people: 2, default_on: false}}));
  show(<DisableProviderDialog provider={provider} open onClose={vi.fn()} />);
  const choice = await screen.findByRole('checkbox', {name: i18n.t('operation.notify', {count:2,number:'2'})});
  expect(choice).not.toBeChecked();
  await user.click(choice);
  await user.click(screen.getByRole('button', {name:i18n.t('providers.danger.disableConfirm', {name:provider.name})}));
  await waitFor(() => expect(calls.disable).toHaveBeenCalledExactlyOnceWith(provider.id, undefined, {notify:true,notification_review:'reviewed-people'}));
});

it('a changed reviewed audience requires the person to choose notification again', async()=>{
  const user=userEvent.setup({delay:null});
  calls.read.mockResolvedValue(Object.assign(impact(),{notification:{preview_id:'first-cohort',people:2,default_on:false}}));
  const client=new QueryClient({defaultOptions:{queries:{retry:false}}});
  show(<DisableProviderDialog provider={provider} open onClose={vi.fn()}/>,client);
  const choice=await screen.findByRole('checkbox',{name:i18n.t('operation.notify',{count:2,number:'2'})});
  await user.click(choice);
  expect(choice).toBeChecked();
  calls.read.mockResolvedValue(Object.assign(impact(),{notification:{preview_id:'second-cohort',people:3,default_on:false}}));
  await act(async()=>{await client.invalidateQueries({queryKey:['pause-dialog']});});
  expect(await screen.findByRole('checkbox',{name:i18n.t('operation.notify',{count:3,number:'3'})})).not.toBeChecked();
});

it('leaves notification off when confirming without choosing its reviewed audience',async()=>{
  const user=userEvent.setup({delay:null});
  calls.read.mockResolvedValue(Object.assign(impact(),{notification:{preview_id:'off-cohort',people:2,default_on:false}}));
  show(<DisableProviderDialog provider={provider} open onClose={vi.fn()}/>);
  expect(await screen.findByRole('checkbox',{name:i18n.t('operation.notify',{count:2,number:'2'})})).not.toBeChecked();
  await user.click(screen.getByRole('button',{name:i18n.t('providers.danger.disableConfirm',{name:provider.name})}));
  await waitFor(()=>expect(calls.disable).toHaveBeenCalledExactlyOnceWith(provider.id));
});

it('keeps a deliberately configured notification default editable for this operation',async()=>{
  const user=userEvent.setup({delay:null});
  calls.read.mockResolvedValue(Object.assign(impact(),{notification:{preview_id:'configured-cohort',people:2,default_on:true}}));
  show(<DisableProviderDialog provider={provider} open onClose={vi.fn()}/>);
  const choice=await screen.findByRole('checkbox',{name:i18n.t('operation.notify',{count:2,number:'2'})});
  expect(choice).toBeChecked();
  await user.click(choice);
  expect(choice).not.toBeChecked();
  await user.click(screen.getByRole('button',{name:i18n.t('providers.danger.disableConfirm',{name:provider.name})}));
  await waitFor(()=>expect(calls.disable).toHaveBeenCalledExactlyOnceWith(provider.id));
});

function applicationImpact(): ResourceImpact {
  const value = impact(), empty = { count: 0, threads: 0, visible: [], hidden: 0, since: null };
  const visible = Array.from({ length: 13 }, (_, index) => ({ ...thread, name: `Affected thread ${index + 1}`,
    object: { kind: 'thread' as const, id: `00000000-0000-4000-8000-${(index + 100).toString().padStart(12, '0')}` } }));
  return { ...value, activity: { running: empty, waiting: empty, stopped: empty }, facts: { ...value.facts, dependents: [] },
    applications: [
      { when: 'now', object_kind: 'thread', count: 14, visible, hidden: 1, apply: null, apply_targets: [] },
      { when: 'before_next_turn', object_kind: 'thread', count: 14, visible, hidden: 1, apply: null, apply_targets: [] },
      { when: 'updated', object_kind: 'environment', count: 3, visible: [], hidden: 3, apply: 'update', apply_targets: [] },
    ] };
}

it.each(['en', 'zh-CN'] as const)('names each application boundary and searches every affected native name without exposing hidden ones (%s)', async (locale) => {
  await setLocale(locale);
  show(<ImpactCounts impact={applicationImpact()} kind="disable" />);
  const now = locale === 'en' ? 'Calls refused now · 14 threads' : '立即拒绝调用 · 14 个线程';
  expect(screen.getByText(now)).toBeVisible();
  expect(screen.getByText(locale === 'en' ? 'Before the next turn · 14 threads' : '下一轮开始前 · 14 个线程')).toBeVisible();
  expect(screen.getByText(locale === 'en' ? 'When updated · 3 environments' : '更新后生效 · 3 个环境')).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('change.env.menu.update') })).toBeVisible();
  await userEvent.click(screen.getByText(now));
  const search = await screen.findByRole('textbox', { name: i18n.t('resourceImpact.search') });
  await userEvent.type(search, 'Affected thread 13');
  expect(screen.getByText('Affected thread 13')).toBeVisible();
  expect(screen.queryByText('Affected thread 1', { exact: true })).not.toBeInTheDocument();
  expect(screen.getByText(i18n.t('storage.hidden', { number: '1' }))).toBeVisible();
});

it('keeps partial native application evidence explicit and omits empty groups', () => {
  const value = applicationImpact();
  show(<ImpactCounts impact={{ ...value, application_complete: false, applications: [] }} kind="disable" />);
  expect(screen.getByText('Some application times are not reported')).toBeVisible();
  expect(screen.queryByText(/0 threads/u)).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('change.env.menu.update') })).not.toBeInTheDocument();
});

it.each(['environment','provider'] as const)('offers the real Update plan for every named manageable %s and leaves hidden targets out',async(kind)=>{
  const value=applicationImpact();
  value.applications=[{when:'updated',object_kind:kind,count:3,hidden:1,
    visible:['first','second'].map((id)=>({object:{kind,id},name:`Visible ${id}`,relation:'attachment',until:null})),
    apply:'update',apply_targets:[{kind,id:'first'},{kind,id:'second'}]}];
  show(<ImpactCounts impact={value} kind="disable"/>);
  await userEvent.click(screen.getByRole('button',{name:i18n.t('change.env.menu.update')}));
  expect(calls.switch).toHaveBeenCalledExactlyOnceWith({kind:kind==='environment'?'environments':'providers',ids:['first','second']},{kind:'update'});
});

it('saves mount availability without waiting for a dependency list or notifying its users', async () => {
  calls.read.mockImplementation(() => new Promise(() => undefined));
  show(<EditMount mount={detailMount} onClose={vi.fn()} />);
  await userEvent.click(screen.getByRole('switch', {name:i18n.t('resourceDetails.enabled')}));
  expect(calls.read).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole('button',{name:i18n.t('resourceImpact.showUses')}));
  await userEvent.click(screen.getByRole('button',{name:i18n.t('resources.edit.save')}));
  await waitFor(() => expect(calls.updateMount).toHaveBeenCalledOnce());
  expect(calls.updateMount.mock.calls[0]?.[1]).not.toHaveProperty('notify');
});

it('retires cached application counts and their names after a fresh impact read is refused', async () => {
  const target: FactTarget = { object: { kind: 'mount', id: detailMount.id }, model_id: null };
  const value = applicationImpact();
  value.target = target;
  value.applications = [{ when: 'updated', object_kind: 'environment', count: 3,
    visible: [{ object: { kind: 'environment', id: 'visible-environment' }, name: 'Private running environment', relation: 'attachment', until: null }], hidden: 2, apply: 'update', apply_targets: [{kind:'environment',id:'visible-environment'}] }];
  calls.read.mockResolvedValue(value);
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  show(<EditMount mount={detailMount} onClose={vi.fn()} />, cache);
  await userEvent.click(screen.getByRole('switch', { name: i18n.t('resourceDetails.enabled') }));
  await userEvent.click(screen.getByRole('button',{name:i18n.t('resourceImpact.showUses')}));
  await userEvent.click(await screen.findByText('When updated · 3 environments'));
  expect(await screen.findByText('Private running environment')).toBeVisible();
  calls.read.mockRejectedValue(new ApiError({ type: 'about:blank', title: 'Access changed', status: 403, code: 'authz.denied',
    detail: 'This impact read is no longer allowed', request_id: 'synthetic' }, null));
  await act(async () => { await cache.invalidateQueries({ queryKey: ['pause-dialog', target], exact: true }); });
  await waitFor(()=>expect(screen.queryByText('Private running environment')).not.toBeInTheDocument());
  expect(screen.queryByText('When updated · 3 environments')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: i18n.t('resources.edit.save') })).not.toHaveAttribute('aria-disabled', 'true');
  expect(calls.updateMount).not.toHaveBeenCalled();
});
