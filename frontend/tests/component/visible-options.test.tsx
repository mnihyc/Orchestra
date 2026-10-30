/** Export content/date choices and the exact audit action are offered directly in both languages. */
import { fireEvent, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';

import { PlaceOptions } from '@/features/export/PlaceOptions';
import { OPENING_CHOICES } from '@/features/export/choices';
import { TraceFilters } from '@/features/trace/Filters';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));

it.each(['en', 'zh-CN'] as const)('shows and changes export content and dates without an extra menu in %s', async (locale) => {
  try {
    await setLocale(locale);
    const change = vi.fn();
    const user = userEvent.setup();
    show(<PlaceOptions choices={OPENING_CHOICES} onChange={change} />);
    expect(screen.getByRole('group', { name: i18n.t('export.options.label') })).toBeVisible();
    expect(screen.getByRole('combobox', { name: i18n.t('export.more.conversations') })).toBeVisible();
    for (const key of ['files', 'versions', 'comments'] as const) expect(screen.getByRole('checkbox', { name: i18n.t(`export.more.${key}`) })).toBeVisible();
    const from = screen.getByLabelText(i18n.t('export.more.from'));
    const to = screen.getByLabelText(i18n.t('export.more.to'));
    expect(from).toBeVisible();
    expect(to).toBeVisible();
    await user.click(screen.getByRole('checkbox', { name: i18n.t('export.more.files') }));
    expect(change).toHaveBeenLastCalledWith({ files: true });
    fireEvent.change(from, { target: { value: '2026-10-01' } });
    expect(change).toHaveBeenLastCalledWith({ from: '2026-10-01' });
    fireEvent.change(to, { target: { value: '2026-10-09' } });
    expect(change).toHaveBeenLastCalledWith({ to: '2026-10-09' });
  } finally { await setLocale('en'); }
});

it.each([['en', 'Permission removed'], ['zh-CN', '已移除权限']] as const)('offers the exact action immediately and keeps the other trace filters in %s', async (locale, actionName) => {
  try {
    await setLocale(locale);
    const change = vi.fn();
    const user = userEvent.setup();
    const filters = { actor: 'reader', kind: 'access', include_agents: true, from: '2026-10-01', to: '2026-10-09' };
    show(<TraceFilters scope={{ type: 'system', name: 'System' }} filters={filters} person={null} onChange={change} />);
    const exactAction = screen.getByRole('combobox', { name: i18n.t('trace.action') });
    expect(exactAction).toBeVisible();
    await user.click(exactAction);
    const action = screen.getByRole('option', { name: actionName });
    expect(action).toHaveTextContent(actionName);
    expect(screen.queryByRole('option', { name: 'permission.removed' })).not.toBeInTheDocument();
    await user.click(action);
    expect(change).toHaveBeenCalledExactlyOnceWith({ ...filters, action: 'permission.removed' });
  } finally { await setLocale('en'); }
});
