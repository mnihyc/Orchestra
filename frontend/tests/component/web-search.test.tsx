import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { EntryFields } from '@/features/resources/providers/ModelEditor';
import { formOf, parametersOf, type EntryForm } from '@/features/resources/providers/entry';
import { ParameterRow } from '@/features/inspector/Parameters';
import type { ModelParameters, ParameterView } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/features/resources/harnesses', () => ({ useHarnessTitle: () => () => 'Harness' }));
afterEach(async () => { await setLocale('en'); });

const model: ModelParameters = { offered: { web_search: ['live'] } };

function Manager({ changed }: { changed: (form: EntryForm) => void }) {
  const [form, setForm] = useState(() => formOf(model));
  return <EntryFields base={model} form={form} onChange={(next) => { setForm(next); changed(next); }} faults={{}} disabled={false} otherGroups={[]} provider={{ kind: 'api', harness_kind: null, protocols: ['anthropic-messages'] }} />;
}

for (const locale of ['en', 'zh-CN'] as const) it(`enables only a declared Messages mode and keeps its default separate (${locale})`, async () => {
  await setLocale(locale);
  const changed = vi.fn<(form: EntryForm) => void>();
  const user = userEvent.setup();
  show(<Manager changed={changed} />);
  const group = screen.getByRole('group', { name: i18n.t('webSearch.title') });
  const live = i18n.t('choiceCatalog.webSearch.live.name');
  expect(within(group).getByRole('switch', { name: live })).not.toBeChecked();
  expect(within(group).queryByRole('switch', { name: i18n.t('choiceCatalog.webSearch.cached.name') })).not.toBeInTheDocument();
  await user.click(within(group).getByRole('switch', { name: live }));
  const selected = changed.mock.lastCall?.[0];
  if (selected === undefined) throw new Error('The manager selection was not retained');
  expect(parametersOf(selected, model)).toMatchObject({ web_search: ['live'] });
  expect(parametersOf(selected, model).default_web_search).toBeUndefined();
});

for (const locale of ['en', 'zh-CN'] as const) it(`uses the resolved choice list, default and existing lock in a thread (${locale})`, async () => {
  await setLocale(locale);
  const parameter: ParameterView = {
    key: 'web_search', value: null, apply: 'new_session', source: { scope: 'default', node: null }, locked_by: null, writable: true,
    def: { title: 'Web search', type: 'enum', options: { from_model: 'web_search' },
      resolved_options: [{ value: 'disabled', label: 'Disabled' }, { value: 'live', label: 'live' }], resolved_default: 'live' },
  };
  const write = vi.fn<(key: string, value: unknown) => Promise<void>>().mockResolvedValue(undefined);
  const runs = { ready: true, write, reset: vi.fn<(key: string) => Promise<void>>().mockResolvedValue(undefined), refuse: vi.fn<() => Promise<void>>().mockResolvedValue(undefined) };
  const access = { thread: { id: 'thread-search', owner: userRef(me(1, 'ada', 'Ada')) } };
  const { rerender } = show(<ul><ParameterRow parameter={parameter} access={access} writable runs={runs} /></ul>);
  const picker = screen.getByRole('combobox', { name: i18n.t('webSearch.title') });
  expect(picker).toHaveTextContent(i18n.t('choiceCatalog.webSearch.live.name'));
  await userEvent.click(picker);
  await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.webSearch.disabled.name') }));
  expect(write).toHaveBeenCalledWith('web_search', 'disabled');
  rerender(<ul><ParameterRow parameter={{ ...parameter, writable: false, locked_by: {scope:'org',node:{id:'provider-policy',kind:'org',name:'Provider policy',redacted:false}} }} access={access} writable runs={runs} /></ul>);
  expect(screen.getByRole('combobox', { name: i18n.t('webSearch.title') })).toBeDisabled();
  expect(screen.getByText(i18n.t('inspector.parameters.lockedBy', {name:'Provider policy'}))).toBeVisible();
});
