/** Partial native sharing and independent role-default reads keep their actual scope and failure state. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Access from '@/api/access';
import { SelectionSharing } from '@/features/everything/Everything';
import { WhatRolesDo } from '@/features/access/Roles';
import type { EverythingRow, PermissionPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { CATALOGUE, pageFor } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ share: vi.fn(), defaults: vi.fn<() => Promise<PermissionPage>>() }));
vi.mock('@/features/access/AccessDialog', () => ({ openAccess: vi.fn(), openAccessToMany: calls.share }));
vi.mock('@/api/access', async (original) => ({ ...await original<typeof Access>(), permissionsQuery: { queryKey: ['role-read-catalogue'], queryFn: () => Promise.resolve(CATALOGUE.reply) },
  defaultsQuery: () => ({ queryKey: ['role-read-defaults'], queryFn: calls.defaults, retry: false }) }));
type Row = Pick<EverythingRow, 'kind' | 'id' | 'name' | 'can_share'>;
const readable: Row = { kind: 'document', id: 'readable', name: 'Readable document', can_share: true };
const unavailable: Row = { kind: 'thread', id: 'unavailable', name: null, can_share: false };
beforeEach(() => { calls.share.mockReset(); calls.defaults.mockReset().mockResolvedValue(pageFor('topic')); });
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it(`names partial sharing before opening only the original native eligible targets (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup();
    show(<SelectionSharing rows={[readable, unavailable]} />);
    expect(screen.getByRole('status')).toHaveTextContent(i18n.t('implicitAvailability.sharingSubset', { count: 1, number: '1', total: '2' }));
    await user.click(screen.getByRole('button', { name: i18n.t('everything.share') }));
    expect(calls.share).toHaveBeenCalledExactlyOnceWith([{ kind: 'document', id: readable.id, name: readable.name }]);
    expect(screen.queryByText(unavailable.id)).not.toBeInTheDocument();
  });
  it.each([false, true])(`explains empty or entirely unshareable selections without opening an empty form (empty=%s, ${locale})`, async (empty) => {
    await setLocale(locale); const user = userEvent.setup();
    show(<SelectionSharing rows={empty ? [] : [unavailable]} />);
    const share = screen.getByRole('button', { name: i18n.t('everything.share') });
    expect(share).toHaveAttribute('aria-disabled', 'true');
    expect(share).toHaveAttribute('aria-description', i18n.t(empty ? 'implicitAvailability.noneSelected' : 'implicitAvailability.noShareable'));
    await user.click(share); expect(calls.share).not.toHaveBeenCalled();
  });
  it(`keeps a failed role-default explanation visible and retries its actual native reader (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup();
    calls.defaults.mockRejectedValueOnce(new NetworkError('offline'));
    show(<WhatRolesDo place={{ kind: 'topic', id: 'place', name: 'Visible place' }} />);
    expect(await screen.findByRole('alert')).toBeVisible();
    await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    expect(calls.defaults).toHaveBeenCalledTimes(2);
  });
}
