/** Preview/copy failures keep their actual cause and recover in the existing form without claiming an absent feature. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as AccessApi from '@/api/access';
import { ApiError } from '@/api/client';
import type * as ShareApi from '@/api/share';
import { ChangePreview, type Asked } from '@/features/access/ChangePreview';
import { ShareWithAgentsDialog } from '@/features/access/ShareWithAgents';
import type { Changes } from '@/features/access/changes';
import type { PagePreview, SharePreview } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { apiErrorSentence } from '@/i18n/errors';

import { CATALOGUE, pageFor } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  read: vi.fn<typeof AccessApi.readPage>(), preview: vi.fn<typeof AccessApi.previewPage>(),
  review: vi.fn<typeof AccessApi.previewShare>(), share: vi.fn<typeof ShareApi.shareWithAgents>(),
}));
vi.mock('@/api/access', async (load) => ({ ...await load<typeof AccessApi>(), readPage: calls.read, previewPage: calls.preview, previewShare: calls.review,
  permissionsQuery: { queryKey: ['access-error-permissions'], queryFn: () => Promise.resolve(CATALOGUE.reply) },
}));
vi.mock('@/api/share', async (load) => ({ ...await load<typeof ShareApi>(), shareWithAgents: calls.share }));
vi.mock('@/features/people/ThreadIdentity', () => ({ ThreadIdentity: () => null }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));

const target = { kind: 'document' as const, id: 'document', name: 'Plan' };
const preview: PagePreview = { pages: 1, orgs: null, gaining: [], losing: [], more: 0 };
const review: SharePreview = { allowed: true, revision: 7, permissions: ['doc.view'], holders: [], effects: preview,
  already: [], already_more: 0, agents: [], rows: [], rules: [], refusal: null };
const missing = () => new ApiError({ type: 'orch:error:not_found', code: 'not_found', status: 404, title: '', detail: 'The reviewed page was removed.', request_id: 'preview-read' }, null);
const unsupported = () => new ApiError({ type: 'orch:error:capability.unsupported', code: 'capability.unsupported', status: 409, title: '', detail: 'The selected program refused this operation.', request_id: 'copy-refusal' }, null);

beforeEach(() => {
  for (const call of Object.values(calls)) call.mockReset();
  calls.read.mockResolvedValue(pageFor('document'));
  calls.preview.mockResolvedValue(preview);
  calls.review.mockResolvedValue(review);
});
afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  for (const failedRead of ['page', 'preview'] as const) it(`retries the actual ${failedRead} failure before confirming (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    if (failedRead === 'page') calls.read.mockRejectedValueOnce(missing());
    else calls.preview.mockRejectedValueOnce(missing());
    const run = vi.fn<Changes['run']>().mockResolvedValue(true);
    const close = vi.fn();
    const asked: Asked = { title: 'Review Plan', body: 'Remove a named entry from Plan.', confirmLabel: 'Remove entry', label: 'Remove entry', make: (page) => ({ expected_revision: page.revision }) };
    const changes: Changes = { busy: false, doing: null, error: null, stale: null, forget: vi.fn(), again: () => Promise.resolve(false), run };
    show(<div data-theme={theme}><ChangePreview source={{ type: 'page', target }} asked={asked} changes={changes} onClose={close} /></div>);
    const error = await screen.findByRole('alert');
    expect(error).toHaveTextContent(apiErrorSentence('not_found', 404, i18n.t, locale));
    const confirm = screen.getByRole('button', { name: asked.confirmLabel });
    expect(confirm).toBeDisabled();
    expect(run).not.toHaveBeenCalled();
    expect(screen.queryByText(i18n.t('access.preview.nobody'), { exact: true })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await screen.findByText(i18n.t('access.preview.nobody'), { exact: true });
    await waitFor(() => expect(confirm).toBeEnabled());
    await userEvent.click(confirm);
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(run).toHaveBeenCalledWith({ label: asked.label, make: asked.make });
    expect(calls.read).toHaveBeenCalledTimes(failedRead === 'page' ? 2 : 1);
    expect(calls.preview).toHaveBeenCalledTimes(failedRead === 'preview' ? 2 : 1);
  });

  it(`keeps a refused copy as Copy, shows the native cause, then retries the same copy (${locale}, ${theme})`, async () => {
    await setLocale(locale);
    const failure = unsupported();
    calls.share.mockRejectedValueOnce(failure).mockResolvedValue({ shares: [] });
    const close = vi.fn();
    show(<div data-theme={theme}><ShareWithAgentsDialog target={target} page={pageFor('document')} open onClose={close} initialRecipient={{ id: 'receiving-thread', name: 'Review thread' }} /></div>);
    await userEvent.click(screen.getByRole('radio', { name: i18n.t('choiceCatalog.agentShareMode.snapshot.name') }));
    const submit = screen.getByRole('button', { name: i18n.t('access.agents.dialog.button.copyOne', { name: 'Review thread' }) });
    await waitFor(() => { expect(submit).toBeEnabled(); expect(submit).not.toHaveAttribute('aria-disabled', 'true'); });
    await userEvent.click(submit);
    const error = await screen.findByRole('alert');
    expect(error).toHaveTextContent(apiErrorSentence('capability.unsupported', 409, i18n.t, locale));
    expect(screen.getByRole('radio', { name: i18n.t('choiceCatalog.agentShareMode.snapshot.name') })).toBeChecked();
    expect(close).not.toHaveBeenCalled();
    const first = calls.share.mock.calls[0];
    expect(first?.[0]).toMatchObject({ what: { document_id: target.id }, mode: 'snapshot', to: [{ thread_id: 'receiving-thread' }] });
    await userEvent.click(submit);
    await waitFor(() => expect(close).toHaveBeenCalledOnce());
    expect(calls.share).toHaveBeenCalledTimes(2);
    expect(calls.share.mock.calls[1]).toEqual(first);
  });
}
