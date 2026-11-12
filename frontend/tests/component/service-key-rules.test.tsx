/** A service rule targets that service account, while the human maker still proves the act. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { TokenForm } from '@/features/settings/TokenForm';
import type { ApiTokenCreateBody } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { TOKEN_OPERATIONS } from '../doubles/token-operations';
import { id, me } from '../doubles/fixtures';
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
