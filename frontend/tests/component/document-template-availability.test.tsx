/** Configured templates keep read failures separate from no templates and from private source data. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@/api/client';
import type * as Documents from '@/api/documents';
import type * as Settings from '@/api/settings';
import { Templates } from '@/features/people/Templates';
import type { Document, ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { show } from './support';

const reads = vi.hoisted(() => ({ settings: vi.fn<() => Promise<ResolvedSetting[]>>(), document: vi.fn<() => Promise<Document>>() }));
vi.mock('@/api/settings', async (original) => ({ ...await original<typeof Settings>(), settingsQuery: () => ({ queryKey: ['template-settings'], queryFn: reads.settings, retry: false }) }));
vi.mock('@/api/documents', async (original) => ({ ...await original<typeof Documents>(), documentQuery: (id: string) => ({ queryKey: ['template-document', id], queryFn: reads.document, retry: false }) }));
const setting: ResolvedSetting = { key: 'docs.templates', value: ['template'], own: null, source: { scope: 'default', node: null }, locked_by: null, writable: false, parts: [] };
const document = { id: 'template', title: 'Readable template', deleted: false } as Document;
beforeEach(() => { reads.settings.mockReset().mockResolvedValue([setting]); reads.document.mockReset().mockResolvedValue(document); });
afterEach(async () => { cleanup(); await setLocale('en'); });
for (const locale of ['en', 'zh-CN'] as const) {
  it.each(['settings', 'document'] as const)(`keeps a failed template %s read visible and retries without starting a copy (${locale})`, async (failed) => {
    await setLocale(locale); const user = userEvent.setup();
    if (failed === 'settings') reads.settings.mockRejectedValueOnce(new NetworkError('offline'));
    else reads.document.mockRejectedValueOnce(new NetworkError('offline'));
    show(<Templates nodeId="place" />);
    expect(await screen.findByText(i18n.t('implicitAvailability.templatesFailed'))).toBeVisible();
    expect(screen.queryByRole('button', { name: document.title })).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByRole('button', { name: document.title })).toBeVisible();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
  it(`keeps private template failures nameless and does not invent an empty configuration (${locale})`, async () => {
    await setLocale(locale);
    reads.document.mockRejectedValue(new ApiError({ type: '', title: 'Denied', status: 403, code: 'authz.denied', detail: 'Hidden template name', request_id: 'template-read' }, null));
    show(<Templates nodeId="place" />);
    expect(await screen.findByText(i18n.t('implicitAvailability.templatesUnavailable'))).toBeVisible();
    expect(screen.queryByText('Hidden template name')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: document.title })).not.toBeInTheDocument();
  });
}
