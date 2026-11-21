/** A service rule targets that service account, while the human maker still proves the act. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import * as directory from '@/api/directory';
import { ServiceAccounts } from '@/features/settings/org/ServiceAccounts';
import { TokenForm } from '@/features/settings/TokenForm';
import type { ApiTokenCreateBody } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { TOKEN_OPERATIONS } from '../doubles/token-operations';
import { id, me, memberAccess, org, userRef } from '../doubles/fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

it('uses the actual service account for This account and keeps the manager’s fresh password', async () => {
  const service = id(0x73, 1), manager = me(1, 'ada', 'Ada'), made = vi.fn<(body: ApiTokenCreateBody) => void>();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  client.setQueryData(['me'], manager);
  client.setQueryData(['token-operations'], TOKEN_OPERATIONS.filter((entry) => entry.operation_id === 'MeTokenList'));
  const shown = show(<QueryClientProvider client={client}><TokenForm operationScoped serviceAccountId={service} formId="service-rule" onMake={made} pending={false} error={null} /></QueryClientProvider>);
  const user = userEvent.setup();
  // The name and the password are what the form holds, not how they were typed: they are pasted, as a keystroke draws the whole form again.
  await user.click(await screen.findByLabelText(i18n.t('tokens.new.name'))); await user.paste('service-reader');
  await user.click(screen.getByRole('button', { name: i18n.t('operationKeys.addRule') }));
  const rule = await screen.findByRole('dialog', { name: i18n.t('operationKeys.addRule') });
  await user.click(within(rule).getByRole('combobox', { name: i18n.t('operationKeys.targetKind') }));
  await user.click(screen.getByRole('option', { name: i18n.t('operationKeys.kind.account') }));
  await user.click(within(rule).getByRole('button', { name: i18n.t('operationKeys.thisAccount') }));
  await user.click(within(rule).getByRole('checkbox', { name: i18n.t('operationKeys.kind.key') }));
  await user.click(within(rule).getByRole('button', { name: i18n.t('operationKeys.preset.read') }));
  await user.click(within(rule).getByRole('button', { name: i18n.t('operationKeys.keepRule') }));
  await user.click(screen.getByLabelText(i18n.t('tokens.new.password'))); await user.paste('human manager proof');
  screen.getByLabelText(i18n.t('tokens.new.name')).closest('form')?.requestSubmit();
  await waitFor(() => expect(made).toHaveBeenCalledOnce());
  expect(made.mock.calls[0]?.[0]).toMatchObject({ rules: [{ target: { kind: 'account', id: service, reach: 'self' }, resources: ['key'], allow: [{ operation: 'MeTokenList', actions: ['read'] }] }], proof: { password: 'human manager proof' } });
  expect(service).not.toBe(manager.id);
  expect(made.mock.calls[0]?.[0]).not.toHaveProperty('scopes');
  shown.unmount(); client.clear();
});

it('asks before revoking the selected service key and lets its manager cancel', async () => {
  const manager = userRef(me(1, 'ada', 'Ada')), service = userRef(me(2, 'builder', 'Build account'));
  const team = org(id(1, 1), 'team', 'Team', manager, { access: { ...memberAccess, permissions: ['members.manage'] } });
  const token = { id: id(0x74, 1), name: 'Build reader', scopes: [], node: null, created_at: '2026-11-01T00:00:00Z', expires_at: null, last_used_at: null };
  const client = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  client.setQueryData(directory.serviceAccountsKey(team.id), [{ user: service, token_count: 1, created_by: manager, created_at: token.created_at }]);
  client.setQueryData(directory.serviceTokensKey(service.id), [token]);
  client.setQueryData(directory.orgTokensKey(team.id), { pages: [{ items: [], next_cursor: null }], pageParams: [null] });
  const revoke = vi.spyOn(directory, 'revokeServiceToken').mockResolvedValue(undefined);
  const shown = show(<ServiceAccounts org={team} />, client), user = userEvent.setup();
  try {
    await user.click(await screen.findByRole('button', { name: i18n.t('settings.service.tokensOf', { name: service.display_name }) }));
    const title = i18n.t('tokens.revokeTitle', { name: token.name });
    const button = await screen.findByRole('button', { name: title });
    await user.click(button);
    const confirmation = await screen.findByRole('alertdialog', { name: title });
    expect(revoke).not.toHaveBeenCalled();
    await user.click(within(confirmation).getByRole('button', { name: i18n.t('common.cancel') }));
    expect(revoke).not.toHaveBeenCalled();
    await user.click(button);
    await user.click(within(await screen.findByRole('alertdialog', { name: title })).getByRole('button', { name: i18n.t('tokens.revokeConfirm', { name: token.name }) }));
    await waitFor(() => expect(revoke).toHaveBeenCalledWith(service.id, token.id));
    expect(revoke).toHaveBeenCalledOnce();
  } finally { shown.unmount(); revoke.mockRestore(); client.clear(); }
});
