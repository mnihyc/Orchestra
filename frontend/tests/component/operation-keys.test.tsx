/** The existing personal token page confirms modern limits and preserves its one-time secret. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { type ReactNode, useState } from 'react';
import type * as Router from '@tanstack/react-router';
import type * as Users from '@/api/users';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Tokens } from '@/features/settings/personal/Tokens';
import { KeyOperationChoice, KeyRuleSummary } from '@/features/settings/KeyRules';
import { DETAIL_PAGE_ROWS } from '@/features/resources/detailPageSize';
import type { ApiTokenCreateBody, ApiTokenInfo, KeyOperation, KeyRule, KeyTargetKind, Operation, TokenCreateResponse } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { me } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ create: vi.fn<(body: ApiTokenCreateBody, key: string) => Promise<TokenCreateResponse>>(), revoke: vi.fn<(id: string) => Promise<void>>() }));
vi.mock('@/api/users', async (load) => ({ ...await load<typeof Users>(), createToken: calls.create, revokeToken: calls.revoke }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), Link: ({ to, children }: { to: string; children: ReactNode }) => <a href={to}>{children}</a> }));

const fresh: TokenCreateResponse = { projection: 'receipt', token_id: '00000000-0000-4000-8001-000000000099', token: 'fixture-first-secret', created_at: '2026-10-01T12:00:00Z', expires_at: null, already_created: false };
const row: ApiTokenInfo = { id: fresh.token_id, name: 'Scoped work', scopes: [], rules: [{ target: { kind: 'project', id: '00000000-0000-4000-8003-000000000002', reach: 'subtree' }, resources: ['document'], allow: [{ operation: 'DocumentOpsCreate', actions: ['write'] }] }], node: null, created_at: fresh.created_at, expires_at: null, last_used_at: null };
const clients: QueryClient[] = [];
function open(rows: ApiTokenInfo[] = []) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity }, mutations: { retry: false } } });
  client.setQueryData(['me'], me(1, 'ada', 'Ada'));
  client.setQueryData(['tokens'], rows);
  client.setQueryData(['permissions'], { permissions: [], choices: [] });
  clients.push(client);
  show(<QueryClientProvider client={client}><Tokens /></QueryClientProvider>);
  return client;
}
beforeEach(() => { calls.create.mockReset().mockResolvedValue(fresh); calls.revoke.mockReset().mockResolvedValue(undefined); });
afterEach(async () => { cleanup(); clients.splice(0).forEach((client) => client.clear()); await setLocale('en'); });

it('opens deny-all rules instead of a future-expanding Everything choice and cancellation creates no key', async () => {
  open();
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('tokens.new.button') }));
  expect(await screen.findByRole('region', { name: i18n.t('operationKeys.rules') })).toHaveTextContent(i18n.t('operationKeys.denyAll'));
  expect(screen.queryByRole('radio', { name: /Everything/ })).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.cancel') }));
  expect(calls.create).not.toHaveBeenCalled();
});

it('submits explicit empty rules with the existing fresh proof and shows the first receipt secret once', async () => {
  const client = open(), user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: i18n.t('tokens.new.button') }));
  await user.type(await screen.findByLabelText(i18n.t('tokens.new.name')), 'receipt-key');
  await user.type(screen.getByLabelText(i18n.t('tokens.new.password')), 'fixture fresh password');
  await user.click(screen.getByRole('button', { name: i18n.t('tokens.new.make') }));
  await waitFor(() => expect(calls.create).toHaveBeenCalledOnce());
  const sent = calls.create.mock.calls[0]?.[0];
  expect(sent).toMatchObject({ rules: [], proof: { password: 'fixture fresh password' } });
  expect(sent).not.toHaveProperty('scopes'); expect(sent).not.toHaveProperty('node_id');
  expect(await screen.findByRole('textbox', { name: i18n.t('tokens.new.secret', { name: 'receipt-key' }) })).toHaveValue(fresh.token);
  expect(client.getMutationCache().getAll().every((mutation) => mutation.state.data === undefined)).toBe(true);
  await user.click(screen.getByRole('button', { name: i18n.t('tokens.new.done') }));
  expect(screen.queryByDisplayValue('fixture-first-secret')).not.toBeInTheDocument();
});

it('keeps a replay receipt secretless and revokes its genuine token ID without resending creation', async () => {
  calls.create.mockResolvedValue({ ...fresh, token: null, already_created: true });
  open(); const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: i18n.t('tokens.new.button') }));
  await user.type(await screen.findByLabelText(i18n.t('tokens.new.name')), 'replayed-key');
  await user.type(screen.getByLabelText(i18n.t('tokens.new.password')), 'fixture fresh password');
  await user.click(screen.getByRole('button', { name: i18n.t('tokens.new.make') }));
  expect(await screen.findByText(i18n.t('tokens.new.alreadyTitle', { name: 'replayed-key' }))).toBeVisible();
  expect(screen.queryByDisplayValue('fixture-first-secret')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('tokens.new.revokeAndAgain') }));
  await waitFor(() => expect(calls.revoke).toHaveBeenCalledWith(fresh.token_id));
  expect(calls.create).toHaveBeenCalledOnce();
});

for (const locale of ['en', 'zh-CN'] as const) it(`reviews exact actions and targets before normal revocation in ${locale}`, async () => {
  await setLocale(locale); open([row]); const user = userEvent.setup();
  const shown = await screen.findByRole('row', { name: /Scoped work/ });
  await user.click(within(shown).getByRole('button', { name: i18n.t('operationKeys.review') }));
  const review = await screen.findByRole('dialog', { name: i18n.t('operationKeys.reviewTitle', { name: row.name }) });
  expect(review).toHaveTextContent('DocumentOpsCreate'); expect(review).toHaveTextContent(i18n.t('operationKeys.action.write'));
  expect(review).toHaveTextContent(row.rules?.[0]?.target.id ?? 'missing target');
  await user.click(within(review).getByText(i18n.t('common.close'), { selector: 'button' }));
  await user.click(within(shown).getByRole('button', { name: i18n.t('tokens.revoke') }));
  const confirm = await screen.findByRole('alertdialog');
  expect(calls.revoke).not.toHaveBeenCalled();
  await user.click(within(confirm).getByRole('button', { name: i18n.t('tokens.revokeConfirm', { name: row.name }) }));
  await waitFor(() => expect(calls.revoke).toHaveBeenCalledWith(row.id));
});

it('keeps only the current saved-operation page mounted without changing the recorded rule', async () => {
  const rule: KeyRule = { target: { kind: 'project', id: '00000000-0000-4000-8003-000000000002', reach: 'subtree' }, resources: ['document'],
    allow: Array.from({ length: DETAIL_PAGE_ROWS + 1 }, (_, index) => ({ operation: `RecordedOperation-${index + 1}`, actions: ['read'] })) };
  const recorded = JSON.stringify(rule);
  show(<section aria-label="Saved rule"><KeyRuleSummary rule={rule} /></section>);
  const summary = within(screen.getByRole('region', { name: 'Saved rule' }));
  expect(summary.getAllByRole('listitem')).toHaveLength(DETAIL_PAGE_ROWS);
  expect(screen.getByText('RecordedOperation-1')).toBeVisible();
  expect(screen.queryByText(`RecordedOperation-${DETAIL_PAGE_ROWS + 1}`)).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('resourceDetails.layout.next') }));
  expect(summary.getAllByRole('listitem')).toHaveLength(1);
  expect(screen.queryByText('RecordedOperation-1')).not.toBeInTheDocument();
  expect(screen.getByText(`RecordedOperation-${DETAIL_PAGE_ROWS + 1}`)).toBeVisible();
  expect(JSON.stringify(rule)).toBe(recorded);
});

it('shows concrete effects and preserves a hidden selected action until it is explicitly removed', async () => {
  const changed = vi.fn<(next: KeyOperation[]) => void>();
  const catalogue: Operation[] = [
    { operation_id: 'DocumentTreeOperate', title: 'Copy documents', method: 'POST', path: '/nodes/{id}/document-tree', delegation: 'business', variants: [{ name: 'copy', operation: 'DocumentTreeOperate.copy', selector: 'copy', legacy_tokens: true, facets: [
      { action: 'read', resources: ['document'], usage: 'required', target: { kind: 'permission_object' } },
      { action: 'write', resources: ['document_container'], usage: 'required', target: { kind: 'resolved', value: 'private_backend_destination_resolver' } },
    ] }] },
    { operation_id: 'DocumentOpsCreate', title: 'Edit documents', method: 'POST', path: '/documents/{id}/ops', delegation: 'business', variants: [{ name: '', operation: 'DocumentOpsCreate', selector: 'all', legacy_tokens: true, facets: [
      { action: 'write', resources: ['document'], usage: 'required', target: { kind: 'permission_object' } },
      { action: 'read', resources: ['document'], usage: 'response_content', target: { kind: 'permission_object' } },
    ] }] },
  ];
  function Choice() {
    const [resources, setResources] = useState<KeyTargetKind[]>(['document', 'document_container']);
    const [value, setValue] = useState<KeyOperation[]>([{ operation: 'DocumentTreeOperate.copy', actions: ['read', 'write'] }]);
    return <><button onClick={() => setResources(['document_container'])}>Use folders only</button><KeyOperationChoice catalogue={catalogue} resources={resources} value={value} onChange={(next) => { changed(next); setValue(next); }} /></>;
  }
  show(<Choice />);
  expect(screen.getByText('DocumentTreeOperate.copy')).toBeVisible();
  expect(screen.getAllByText(new RegExp(i18n.t('operationKeys.usage.required'))).length).toBeGreaterThan(0);
  expect(screen.getByText(new RegExp(i18n.t('operationKeys.usage.response_content')))).toBeVisible();
  expect(screen.queryByText(/private_backend_destination_resolver/)).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Use folders only' }));
  expect(changed).not.toHaveBeenCalled();
  const warning = await screen.findByRole('alert');
  expect(warning).toHaveTextContent('DocumentTreeOperate.copy');
  expect(warning).toHaveTextContent(i18n.t('operationKeys.action.read'));
  expect(screen.queryByRole('checkbox', { name: i18n.t('operationKeys.action.read') })).not.toBeInTheDocument();
  await userEvent.click(within(warning).getByRole('button', { name: i18n.t('operationKeys.removeUnavailableFor', { operation: 'DocumentTreeOperate.copy' }) }));
  expect(changed).toHaveBeenLastCalledWith([{ operation: 'DocumentTreeOperate.copy', actions: ['write'] }]);
  expect(screen.getByRole('checkbox', { name: i18n.t('operationKeys.action.write') })).toBeChecked();
});
