/** Managed search uses the supplied ordinary choices without choosing a paid service on opening. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ParameterRow } from '@/features/inspector/Parameters';
import type { ParameterView } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

vi.mock('@/features/resources/harnesses', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHarnessTitle: () => () => 'Claude Code' }));
afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`selects service explicitly without native modes and retains the higher lock (${locale})`, async () => {
  await setLocale(locale);
  const parameter: ParameterView = {
    key: 'web_search', value: null, apply: 'new_session', source: { scope: 'default', node: null }, locked_by: null, writable: true,
    def: { title: 'Web search', type: 'enum', group: 'behaviour', capability: 'web_search', native_constraint: false,
      resolved_options: [{ value: 'disabled', label: 'Disabled' }, { value: 'service', label: 'Service' }], resolved_default: null },
  };
  const write = vi.fn<(key: string, value: unknown) => Promise<void>>().mockResolvedValue(undefined);
  const runs = { ready: true, write, reset: vi.fn<() => Promise<void>>().mockResolvedValue(undefined), refuse: vi.fn<() => Promise<void>>().mockResolvedValue(undefined) };
  const access = { thread: { id: 'managed-search-thread', owner: userRef(me(1, 'ada', 'Ada')) } };
  const { rerender } = show(<ul><ParameterRow parameter={parameter} access={access} writable runs={runs} /></ul>);
  const user = userEvent.setup();
  const picker = screen.getByRole('combobox', { name: i18n.t('webSearch.title') });
  expect(picker).toHaveValue('');
  expect(write).not.toHaveBeenCalled();
  await user.click(picker);
  expect(screen.getAllByRole('option')).toHaveLength(2);
  for (const native of ['live', 'cached', 'indexed'] as const) {
    expect(screen.queryByRole('option', { name: i18n.t(`choiceCatalog.webSearch.${native}.name`) })).not.toBeInTheDocument();
  }
  expect(write).not.toHaveBeenCalled();
  await user.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.webSearch.service.name') }));
  expect(write).toHaveBeenCalledExactlyOnceWith('web_search', 'service');
  rerender(<ul><ParameterRow parameter={{ ...parameter, value: 'service', writable: false,
    locked_by: { scope: 'org', node: { id: 'search-policy', kind: 'org', name: 'Search policy', redacted: false } } }} access={access} writable runs={runs} /></ul>);
  expect(picker).toHaveValue(i18n.t('choiceCatalog.webSearch.service.name'));
  expect(picker).toBeDisabled();
  expect(write).toHaveBeenCalledTimes(1);
});
