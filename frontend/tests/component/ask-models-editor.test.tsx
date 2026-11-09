import { cleanup, fireEvent, screen } from '@testing-library/react';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { AskModelsEditor } from '@/features/settings/AskModelsEditor';
import type { ModelEffortEntry } from '@/features/composer/ModelEffort';
import type { ModelIdentity } from '@/features/composer/modelSelection';
import { i18n, setLocale } from '@/i18n';
import { askModels } from '../unit/ask-fixtures';
import { show } from './support';

function entries(): ModelEffortEntry[] {
  const configured = askModels().models[0];
  if (configured === undefined || configured.model === null || configured.provider === null) throw new Error('The editor fixture needs one allowed model.');
  const option = configured.model, provider = configured.provider;
  return [
    { id: 'first', option, providerName: provider.name, value: { provider_id: provider.id, model: option.model_name, params: { effort: 'high' } } },
    { id: 'second', option: { ...option, model_name: 'second', display_name: 'Second API model' }, providerName: provider.name,
      value: { provider_id: provider.id, model: 'second', params: { temperature: 0.2 } } },
  ];
}
function Editor({ mode = 'system' }: { mode?: 'system' | 'narrow' }) {
  const choices = entries(), first = choices[0];
  if (first === undefined) throw new Error('The initial row is required.');
  const identities = choices.map(({ value }) => ({ provider_id: value.provider_id, model: value.model }));
  const [value, setValue] = useState<readonly ModelIdentity[]>(mode === 'system' ? identities.slice(0, 1) : identities);
  return <><AskModelsEditor label="Ordered models" value={value} entries={choices} mode={mode} onChange={setValue} /><output aria-label="Stored models">{JSON.stringify(value)}</output></>;
}
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`adds, reorders and removes system identities without storing effort (${locale})`, async () => {
    await setLocale(locale); const t = i18n.getFixedT(locale), choices = entries(); show(<Editor />);
    fireEvent.click(screen.getByRole('button', { name: t('modelList.add') }));
    expect(screen.queryByRole('group', { name: /effort/i })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('menuitemradio', { name: 'Second API model' }));
    const identities = choices.map(({ value }) => ({ provider_id: value.provider_id, model: value.model }));
    expect(JSON.parse(screen.getByRole('status', { name: 'Stored models' }).textContent ?? 'null')).toEqual(identities);
    fireEvent.click(screen.getByRole('button', { name: t('modelList.moveUp', { model: t('modelList.entry', { number: '2' }) }) }));
    expect(JSON.parse(screen.getByRole('status', { name: 'Stored models' }).textContent ?? 'null')).toEqual([identities[1], identities[0]]);
    fireEvent.click(screen.getByRole('button', { name: t('modelList.remove', { model: t('modelList.entry', { number: '2' }) }) }));
    expect(JSON.parse(screen.getByRole('status', { name: 'Stored models' }).textContent ?? 'null')).toEqual([identities[1]]);
  });
  it(`allows an org to remove only and states that the order comes from above (${locale})`, async () => {
    await setLocale(locale); const t = i18n.getFixedT(locale), choices = entries(); show(<Editor mode="narrow" />);
    expect(screen.getByText(t('ask.modelsFromAbove'))).toBeVisible();
    expect(screen.queryByRole('button', { name: t('modelList.add') })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: t('modelList.moveUp', { model: t('modelList.entry', { number: '2' }) }) })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: t('modelList.entry', { number: '1' }) })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: t('modelList.remove', { model: t('modelList.entry', { number: '1' }) }) }));
    const second = choices[1]?.value;
    expect(JSON.parse(screen.getByRole('status', { name: 'Stored models' }).textContent ?? 'null')).toEqual([{ provider_id: second?.provider_id, model: second?.model }]);
  });
  it(`does not edit a locked ordered setting (${locale})`, async () => {
    await setLocale(locale); const change = vi.fn(), choices = entries();
    show(<AskModelsEditor label="Ordered models" value={choices.map((entry) => entry.value)} entries={choices} disabled onChange={change} />);
    for (const button of screen.getAllByRole('button')) expect(button).toBeDisabled();
    expect(change).not.toHaveBeenCalled();
  });
}
