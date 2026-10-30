/** Resource confirmations wait for complete effects, retain emergency-stop intent and keep switching separate. */
import tokenDensity from '../../../crates/control/engine/data/token-density.json' with {type:'json'};
import type * as ResourceApi from '@/api/resources';
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { DeleteProviderDialog, DisableProviderDialog } from '@/features/resources/providers/DangerZone';
import { ModelLifecycle } from '@/features/resources/providers/ModelLifecycle';
import type { FactTarget, ProviderModel, ResourceImpact } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ read: vi.fn<(target: FactTarget) => Promise<ResourceImpact>>(), disable: vi.fn(), remove: vi.fn(), updateModel:vi.fn(),removeModel:vi.fn(), switch: vi.fn() }));
vi.mock('@/api/resource-impact', () => ({ resourceImpactQuery: (target: FactTarget) => ({ queryKey: ['pause-dialog', target], queryFn: () => calls.read(target), retry: false }) }));
vi.mock('@/api/resources', async (original) => ({ ...await original<typeof ResourceApi>(), disableProvider: calls.disable, deleteProvider: calls.remove,updateModel:calls.updateModel,deleteModel:calls.removeModel }));
vi.mock('@/features/change/target', () => ({ openChange: calls.switch }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

const provider = planProvider();
const model:ProviderModel={id:'model',model_name:'route-model',display_name:'Research model',enabled:true,source:'manual',groups:[],parameters:{},token_estimate:{...tokenDensity.unmeasured,measured:false}};
const thread = { object: { kind: 'thread' as const, id: '00000000-0000-4000-8000-000000000101' }, name: 'Repair redirects', relation: 'binding' as const, until: null };
function impact(): ResourceImpact {
  const empty = { count: 0, threads: 0, visible: [], hidden: 0, since: null };
  return { target: { object: { kind: 'provider', id: provider.id }, model_id: null }, name: provider.name, stopping: false,
    activity: { running: { count: 2, threads: 2, visible: [thread], hidden: 1, since: null }, waiting: empty, stopped: empty },
    facts: { subject: { kind: 'resource', object: { kind: 'provider', id: provider.id } }, as_of: new Date().toISOString(), dependents_complete: true, uses: null,
      dependents: [{ relation: 'binding', count: 5, visible: [thread], hidden: 4, until: null }] },
  };
}
beforeEach(() => { calls.read.mockResolvedValue(impact()); calls.disable.mockResolvedValue(provider); calls.remove.mockResolvedValue(undefined); calls.updateModel.mockResolvedValue(model);calls.removeModel.mockResolvedValue(undefined); });
afterEach(async () => { cleanup(); vi.resetAllMocks(); await setLocale('en'); });

it('waits for the current effects before admitting an emergency stop and sends only its one-line note', async () => {
  const user = userEvent.setup();
  let ready: ((value: ResourceImpact) => void) | undefined;
  calls.read.mockImplementation(() => new Promise<ResourceImpact>((resolve) => { ready = resolve; }));
  show(<DisableProviderDialog provider={provider} open onClose={vi.fn()} />);
  const confirm = screen.getByRole('button', { name: i18n.t('providers.danger.disableConfirm', { name: provider.name }) });
  expect(confirm).toBeDisabled();
  expect(calls.disable).not.toHaveBeenCalled();
  await waitFor(() => expect(ready).toBeTypeOf('function'));
  ready?.(impact());
  expect(await screen.findByText('2 turns stop now')).toBeVisible();
  expect(screen.getByText('5 threads stay bound')).toBeVisible();
  expect(screen.queryByRole('combobox')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('resources.act.switchThreads') })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('resourceImpact.switchFirst') })).not.toBeInTheDocument();
  await user.type(screen.getByRole('textbox', { name: new RegExp(i18n.t('resourceImpact.note')) }), '  Key rotation  ');
  await user.click(confirm);
  await waitFor(() => expect(calls.disable).toHaveBeenCalledExactlyOnceWith(provider.id, 'Key rotation'));
});

it('a failed impact read cannot silently turn unknown effects into zero or permit confirmation', async () => {
  calls.read.mockRejectedValue(new Error('The current dependencies could not be read.'));
  show(<DisableProviderDialog provider={provider} open onClose={vi.fn()} />);
  await screen.findByRole('button', { name: i18n.t('common.tryAgain') });
  expect(screen.getByRole('button', { name: i18n.t('providers.danger.disableConfirm', { name: provider.name }) })).toBeDisabled();
  expect(calls.disable).not.toHaveBeenCalled();
  expect(screen.queryByText('0 turns stop now')).not.toBeInTheDocument();
});

it('deleting requires the current exact name and offers thread switching as an independent act', async () => {
  const user = userEvent.setup();
  const close = vi.fn();
  show(<DeleteProviderDialog provider={provider} open onClose={close} />);
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
  await waitFor(()=>expect(calls.updateModel).toHaveBeenCalledExactlyOnceWith(provider.id,model.id,{enabled:false},'Model pause'));
});

it('a model deletion requires its exact current name before ending its membership',async()=>{
  const user=userEvent.setup({delay:null});
  calls.read.mockResolvedValue({...impact(),name:model.display_name,target:{object:{kind:'provider',id:provider.id},model_id:model.id}});
  show(<ModelLifecycle provider={provider} model={model} kind="delete" onClose={vi.fn()} />);
  await screen.findByText('5 threads stay bound');
  const submit=screen.getByRole('button',{name:i18n.t('providers.danger.deleteConfirm',{name:model.display_name})});
  expect(submit).toBeDisabled();
  expect(calls.removeModel).not.toHaveBeenCalled();
  await user.type(screen.getByRole('textbox',{name:i18n.t('resourceImpact.typeName',{name:model.display_name})}),model.display_name);
  await user.click(submit);
  await waitFor(()=>expect(calls.removeModel).toHaveBeenCalledExactlyOnceWith(provider.id,model.id));
});
