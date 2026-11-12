/** Existing cards and rule inspectors keep native read failures separate from private and empty results. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as AccessApi from '@/api/access';
import type * as ClientApi from '@/api/client';
import { ApiError } from '@/api/client';
import { CardSharing } from '@/features/access/CardSharing';
import { ThreadRules } from '@/features/approvals/ThreadRules';
import type { PermissionPage, ThreadApprovalRule } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { CATALOGUE, pageFor } from '../unit/general-access-fixtures';
import { show } from './support';

const reads = vi.hoisted(() => ({ page: vi.fn<() => Promise<PermissionPage>>(), rules: vi.fn<() => Promise<ThreadApprovalRule[]>>() }));
vi.mock('@/api/access', async (original) => ({ ...await original<typeof AccessApi>(),
  pageQuery: () => ({ queryKey: ['availability-card-page'], initialData: pageFor('document'), queryFn: reads.page, retry: false }),
  permissionsQuery: { queryKey: ['availability-card-catalogue'], initialData: () => CATALOGUE.reply, queryFn: () => Promise.resolve(CATALOGUE.reply) },
}));
vi.mock('@/api/client', async (original) => {
  const client = await original<typeof ClientApi>();
  return { ...client, api: { ...client.api, GET: (path: string) => {
    if (path !== '/threads/{id}/approval-rules') throw new Error(`Unexpected native reader: ${path}`);
    return reads.rules().then((items) => ({ data: { items, next_cursor: null }, response: new Response(null, { status: 200 }) }));
  } } };
});
beforeEach(() => { reads.page.mockReset().mockResolvedValue(pageFor('document')); reads.rules.mockReset().mockResolvedValue([]); });
afterEach(async () => { cleanup(); await setLocale('en'); });
const target = { kind: 'document' as const, id: 'readable-document', name: 'Readable document' };
const privateRead = () => new ApiError({ type: '', title: 'Denied', status: 403, code: 'authz.denied', detail: 'Hidden person and rule', request_id: 'private-read' }, null);

for (const locale of ['en', 'zh-CN'] as const) {
  it(`withdraws a cached card audience until the actual sharing reader retries (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup(); reads.page.mockRejectedValueOnce(new NetworkError('offline'));
    show(<CardSharing target={target} />);
    expect(await screen.findByText(i18n.t('linkCards.sharingUnavailable'))).toBeVisible();
    expect(screen.getByRole('button', { name: i18n.t('linkCards.share') })).toHaveAttribute('aria-disabled', 'true');
    await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByRole('button', { name: i18n.t('linkCards.share') })).toBeVisible();
    expect(reads.page).toHaveBeenCalledTimes(2);
  });
  it(`keeps a private card audience generic and names no old sharing facts (${locale})`, async () => {
    await setLocale(locale); reads.page.mockRejectedValue(privateRead());
    show(<CardSharing target={target} />);
    expect(await screen.findByText(i18n.t('linkCards.sharingRestricted'))).toBeVisible();
    expect(screen.getByRole('button', { name: i18n.t('linkCards.share') })).toHaveAttribute('aria-disabled', 'true');
    expect(screen.queryByText('Hidden person and rule')).not.toBeInTheDocument();
  });
  it(`retries approval rules instead of drawing the read failure as no rules (${locale})`, async () => {
    await setLocale(locale); const user = userEvent.setup(); reads.rules.mockRejectedValueOnce(new NetworkError('offline'));
    show(<ThreadRules threadId="thread" writable />);
    expect(await screen.findByText(i18n.t('approvals.rules.unavailable'))).toBeVisible();
    expect(screen.queryByText(i18n.t('approvals.rules.none'))).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByText(i18n.t('approvals.rules.none'))).toBeVisible();
    expect(reads.rules).toHaveBeenCalledTimes(2);
  });
  it(`does not lend cached rules or private error details after a native refusal (${locale})`, async () => {
    await setLocale(locale); reads.rules.mockRejectedValue(privateRead());
    show(<ThreadRules threadId="thread" writable />);
    expect(await screen.findByText(i18n.t('implicitAvailability.detailsUnavailable'))).toBeVisible();
    expect(screen.queryByText('Hidden person and rule')).not.toBeInTheDocument();
    expect(screen.queryByText(i18n.t('approvals.rules.none'))).not.toBeInTheDocument();
  });
  it(`keeps private rules unread when Manage is not held (${locale})`, async () => {
    await setLocale(locale);
    const view = show(<ThreadRules threadId="thread" writable={false} />);
    await waitFor(() => expect(view.container.querySelector('[data-approval-rules]')).toBeNull());
    expect(reads.rules).not.toHaveBeenCalled();
  });
}
