/** Native indexed mode is an explicit resolved choice; it never enables itself. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ParameterRow } from '@/features/inspector/Parameters';
import { i18n, setLocale } from '@/i18n';
import type { ParameterView } from '@/generated/api-types';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

afterEach(async () => { await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) it(`selects the supplied indexed mode without changing the ordinary default (${locale})`, async () => {
  await setLocale(locale);
  const parameter: ParameterView = { key: 'web_search', value: null, apply: 'new_session', source: { scope: 'default', node: null }, locked_by: null, writable: true,
    def: { title: 'Web search', type: 'enum', options: { from_model: 'web_search' }, resolved_options: [{ value: 'disabled', label: 'Disabled' }, { value: 'indexed', label: 'indexed' }], resolved_default: 'disabled' } };
  const write = vi.fn<(key: string, value: unknown) => Promise<void>>().mockResolvedValue(undefined);
  const runs = { ready: true, write, reset: vi.fn<() => Promise<void>>().mockResolvedValue(undefined), refuse: vi.fn<() => Promise<void>>().mockResolvedValue(undefined) };
  show(<ul><ParameterRow parameter={parameter} access={{ thread: { id: 'search-thread', owner: userRef(me(1, 'ada', 'Ada')) } }} writable runs={runs} /></ul>);
  const picker = screen.getByRole('combobox', { name: i18n.t('webSearch.title') });
  expect(picker).toHaveValue(i18n.t('choiceCatalog.webSearch.disabled.name'));
  expect(write).not.toHaveBeenCalled();
  await userEvent.click(picker);
  await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.webSearch.indexed.name') }));
  expect(write).toHaveBeenCalledExactlyOnceWith('web_search', 'indexed');
});
