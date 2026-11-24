/** Ordered Ask policy retains the selected backend; parameters belong to the message composer. */
import { QueryClient } from '@tanstack/react-query';
import { act, cleanup, fireEvent, screen } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { poolQuery } from '@/api/pools';
import { AskModelsEditor } from '@/features/settings/AskModelsEditor';
import type { AskTarget } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { apiAskModel } from '../unit/ask-fixtures';
import { show } from './support';

const first = apiAskModel('team', 'model');
const second = { backend: { route: 'ask.delegation', offering: first.backend.offering, config: { environment_id: 'env', helper_topic_id: 'topic' } } };
vi.mock('@/features/settings/CapabilityBackendChoice', () => ({
  CapabilityOfferingChoice: ({ label, onChange, disabled, modelPools }: { modelPools?: {onSelect: (id: string, name: string) => void}; label: string; onChange: (value: unknown) => void; disabled: boolean }) =>
    <><button type="button" disabled={disabled} aria-label={label + ' choice'} onClick={() => onChange(second.backend)}>Shared program</button>
    {modelPools && <button type="button" aria-label={label + ' pool'} onClick={() => modelPools.onSelect('review', 'Review models')}>Review models</button>}</>,
  CapabilityOfferingName: () => <span>Configured offering</span>,
}));
function Editor({ narrow = false, disabled = false }: { narrow?: boolean; disabled?: boolean }) {
  const [value, setValue] = useState<AskTarget[]>(narrow ? [first, second] : [first]);
  return <><AskModelsEditor label="Ordered models" target={narrow ? { kind: 'node', id: 'org' } : { kind: 'system' }} disabled={disabled} value={value} onChange={setValue} />
    <output aria-label="Stored models">{JSON.stringify(value)}</output></>;
}
const stored = (): unknown => JSON.parse(screen.getByRole('status', { name: 'Stored models' }).textContent);
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`adds and reorders two exact backends for the same offering in ${locale}`, async () => {
    await setLocale(locale); const t = i18n.getFixedT(locale); show(<Editor />);
    fireEvent.click(screen.getByRole('button', { name: t('modelList.add') }));
    fireEvent.click(screen.getByRole('button', { name: t('modelList.add') + ' choice' }));
    expect(stored()).toEqual([first, second]);
    fireEvent.click(screen.getByRole('button', { name: t('modelList.moveUp', { model: t('modelList.entry', { number: '2' }) }) }));
    expect(stored()).toEqual([second, first]);
    fireEvent.click(screen.getByRole('button', { name: t('modelList.remove', { model: t('modelList.entry', { number: '2' }) }) }));
    expect(stored()).toEqual([second]);
  });
  it(`allows an org to remove whole entries in parent order in ${locale}`, async () => {
    await setLocale(locale); const t = i18n.getFixedT(locale); show(<Editor narrow />);
    expect(screen.getByText(t('ask.modelsFromAbove'))).toBeVisible();
    expect(screen.queryByRole('button', { name: t('modelList.add') })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: t('modelList.moveUp', { model: t('modelList.entry', { number: '2' }) }) })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: t('modelList.remove', { model: t('modelList.entry', { number: '1' }) }) }));
    expect(stored()).toEqual([second]);
  });
  it(`keeps a locked policy read-only in ${locale}`, async () => {
    await setLocale(locale); show(<Editor disabled />);
    for (const button of screen.getAllByRole('button')) expect(button).toBeDisabled();
    expect(stored()).toEqual([first]);
  });
}

it('finds an entry beyond the first page without mounting every model picker', () => {
  const rows=Array.from({length:40}, (_,index) => apiAskModel('team', `listed-model-${index}`));
  show(<AskModelsEditor label="Policy" target={{kind:'system'}} value={rows} onChange={vi.fn()} />);
  expect(screen.getAllByRole('listitem').length).toBeLessThan(rows.length);
  fireEvent.change(screen.getByRole('textbox',{name:i18n.t('composer.model.find')}),{target:{value:'listed-model-39'}});
  expect(screen.getAllByRole('listitem')).toHaveLength(1);
  expect(screen.getByRole('listitem')).toHaveAccessibleName(i18n.t('modelList.entry',{number:'40'}));
});

it('saves an Ask model pool in order alongside exact program and API choices', () => {
  show(<Editor />);
  fireEvent.click(screen.getByRole('button', {name: i18n.t('modelList.add')}));
  fireEvent.click(screen.getByRole('button', {name: i18n.t('modelList.add') + ' pool'}));
  expect(stored()).toEqual([first, {target: {type: 'pool', kind: 'model', id: 'review'}}]);
});

it('opens inherited Ask pools from the current reader’s names without looking up the pool list', async () => {
  const before = client.session.getState();
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const id = '00000000-0000-4000-8000-000000000001';
  client.session.setState({userId: 'reader', readerVersion: 1});
  cache.setQueryData(poolQuery(id, 'reader:1').queryKey, {
    id, name: 'Editorial models', description: '', address: '', kind: 'model', enabled: true,
    policy: {type: 'ordered'}, lend_members: false, revision: 1, owner: null, home: null, settings: [],
    target: {type: 'pool', kind: 'model', id},
    grouping: {home: null, home_path: [], origin_machine: null, availability: []},
    access: {permissions: ['pool.use', 'pool.view'], administers: false, through: []},
  });
  try {
    show(<AskModelsEditor label="Policy" target={{kind: 'node', id: 'org'}}
      value={[{target: {type: 'pool', kind: 'model', id}}]} onChange={vi.fn()} />, cache);
    expect(screen.getByText('Editorial models')).toBeVisible();
    await act(async () => { client.session.setState({readerVersion: 2}); });
    expect(screen.queryByText('Editorial models')).not.toBeInTheDocument();
    expect(screen.getByText(i18n.t('pools.selectedModel'))).toBeVisible();
    expect(cache.getQueryCache().findAll({ queryKey: ['resource-target-pools'] })
      .some((query) => query.state.fetchStatus !== 'idle' || query.state.dataUpdatedAt > 0 || query.state.errorUpdatedAt > 0)).toBe(false);
  } finally { cleanup(); cache.clear(); client.session.setState(before); }
});
