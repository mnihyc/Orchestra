/** The real schema form keeps inheritance, explicit writes and reader/document boundaries in both languages. */
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { DocumentSummaryOptionsDialog } from '@/features/summaries/DocumentOptions';
import type { SummaryOptionsReceipt, SummaryOptionsView, SummaryTargetOptions } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const source = vi.hoisted(() => ({
  read: vi.fn<(id: string) => Promise<SummaryOptionsView>>(),
  write: vi.fn<(id: string, options: SummaryTargetOptions | null, revision: number) => Promise<SummaryOptionsReceipt>>(),
}));
vi.mock('@/api/summaries', async (original) => {
  const actual = await original<typeof import('@/api/summaries')>();
  return { ...actual, documentSummaryOptionsQuery: (id: string) => ({ queryKey: actual.documentSummaryOptionsKey(id), queryFn: () => source.read(id), retry: false }),
    setDocumentSummaryOptions: source.write };
});
const previous = client.session.getState();
function view(id = 'document-a', patch: Partial<SummaryOptionsView> = {}): SummaryOptionsView {
  return { target: { kind: 'document', id }, options: { automatic: false }, min_interval_secs: 300, own: null, revision: 0, writable: true, locked: false, ...patch };
}
const receipt = (id: string, revision: number): SummaryOptionsReceipt => ({ projection: 'receipt', target: { kind: 'document', id }, revision });
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
beforeEach(() => {
  vi.clearAllMocks();
  client.session.setState({ status: 'signed_in', userId: 'summary-reader-a', readerVersion: 40 });
  source.read.mockImplementation(async (id) => view(id));
  source.write.mockImplementation(async (id, options, revision) => {
    source.read.mockResolvedValue(view(id, { options: options ?? { automatic: false }, own: options, revision: revision + 1 }));
    return receipt(id, revision + 1);
  });
});
afterEach(async () => { cleanup(); client.session.setState(previous, true); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`edits the actual document entry, receives a receipt and refetches independently (${locale})`, async () => {
    await setLocale(locale);
    const close = vi.fn();
    show(<DocumentSummaryOptionsDialog documentId="document-a" onOpenChange={close} />);
    await screen.findByText(i18n.t('summaryOptions.document.inherited'));
    expect(screen.getByText(i18n.t('summaryOptions.document.interval', { seconds: 300 }))).toBeVisible();
    expect(source.write).not.toHaveBeenCalled();
    const user = userEvent.setup();
    const set = (field: string) => i18n.t('settings.editor.memberSet', { name: i18n.t(`summaryOptions.fields.${field}`) });
    await user.click(screen.getByRole('checkbox', { name: set('on') }));
    await user.click(screen.getByRole('checkbox', { name: i18n.t('choiceCatalog.automationTrigger.content_changed.name') }));
    await user.click(screen.getByRole('checkbox', { name: set('output_chars') }));
    const output = screen.getByRole('spinbutton', { name: i18n.t('summaryOptions.fields.output_chars') });
    await user.clear(output);
    await user.click(screen.getByRole('button', { name: i18n.t('common.save') }));
    expect(source.write).not.toHaveBeenCalled();
    await user.type(output, '1200');
    expect(screen.getByRole('switch', { name: i18n.t('summaryOptions.fields.automatic') })).not.toBeChecked();
    await user.click(screen.getByRole('button', { name: i18n.t('common.save') }));
    await waitFor(() => expect(close).toHaveBeenCalledWith(false));
    expect(source.write).toHaveBeenCalledExactlyOnceWith('document-a', { automatic: false, on: ['content_changed'], output_chars: 1200 }, 0);
    expect(source.read.mock.calls.length).toBeGreaterThan(1);
  });

  it(`resets an own choice only through the explicit default act (${locale})`, async () => {
    await setLocale(locale);
    source.read.mockResolvedValue(view('document-a', { options: { automatic: true, on: [], min_interval_secs: 0 }, min_interval_secs: 0, own: { automatic: true, on: [], min_interval_secs: 0 }, revision: 7 }));
    const close = vi.fn();
    show(<DocumentSummaryOptionsDialog documentId="document-a" onOpenChange={close} />);
    await screen.findByText(i18n.t('summaryOptions.document.own'));
    expect(screen.getByText(i18n.t('summaryOptions.document.interval', { seconds: 0 }))).toBeVisible();
    expect(screen.getByRole('spinbutton', { name: i18n.t('summaryOptions.fields.min_interval_secs') })).toHaveValue(0);
    expect(source.write).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryOptions.document.useDefault') }));
    await waitFor(() => expect(close).toHaveBeenCalledWith(false));
    expect(source.write).toHaveBeenCalledExactlyOnceWith('document-a', null, 7);
  });

  it.each(['locked', 'readOnly'] as const)(`keeps %s options visible without a write (${locale})`, async (state) => {
    await setLocale(locale);
    source.read.mockResolvedValue(view('document-a', { options: { automatic: true }, own: { automatic: false }, writable: false, locked: state === 'locked' }));
    show(<DocumentSummaryOptionsDialog documentId="document-a" onOpenChange={vi.fn()} />);
    await screen.findByText(i18n.t(`summaryOptions.document.${state}`));
    expect(screen.getByRole('switch', { name: i18n.t('summaryOptions.fields.automatic') })).toBeChecked();
    expect(screen.getByRole('switch', { name: i18n.t('summaryOptions.fields.automatic') })).toBeDisabled();
    expect(screen.getByRole('button', { name: i18n.t('common.save') })).toBeDisabled();
    if (state === 'locked') expect(screen.queryByRole('button', { name: i18n.t('summaryOptions.document.useDefault') })).not.toBeInTheDocument();
    expect(source.write).not.toHaveBeenCalled();
  });

  it(`loads with shared placeholders and retries a failed read explicitly (${locale})`, async () => {
    await setLocale(locale);
    const read = deferred<SummaryOptionsView>();
    source.read.mockReturnValueOnce(read.promise);
    const page = show(<DocumentSummaryOptionsDialog documentId="document-a" onOpenChange={vi.fn()} />);
    expect(page.container.querySelector('[data-skeleton-row]') ?? document.querySelector('[aria-busy="true"]')).not.toBeNull();
    await act(async () => { read.reject(new Error('Synthetic read failure')); await read.promise.catch(() => undefined); });
    await screen.findByRole('alert');
    expect(source.read).toHaveBeenCalledTimes(1);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await screen.findByText(i18n.t('summaryOptions.document.inherited'));
    expect(source.write).not.toHaveBeenCalled();
  });

  it(`locks one pending save, retains its error and reloads only when asked (${locale})`, async () => {
    await setLocale(locale);
    const write = deferred<SummaryOptionsReceipt>();
    source.write.mockReturnValueOnce(write.promise);
    show(<DocumentSummaryOptionsDialog documentId="document-a" onOpenChange={vi.fn()} />);
    await screen.findByText(i18n.t('summaryOptions.document.inherited'));
    const user = userEvent.setup();
    await user.click(screen.getByRole('switch', { name: i18n.t('summaryOptions.fields.automatic') }));
    const save = screen.getByRole('button', { name: i18n.t('common.save') });
    await user.click(save);
    expect(save).toBeDisabled();
    await user.click(save);
    expect(source.write).toHaveBeenCalledTimes(1);
    await act(async () => { write.reject(new Error('Synthetic save failure')); await write.promise.catch(() => undefined); });
    await screen.findByRole('alert');
    expect(source.write).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: i18n.t('summaryOptions.document.reload') }));
    await waitFor(() => expect(screen.queryByRole('alert')).not.toBeInTheDocument());
    expect(source.write).toHaveBeenCalledTimes(1);
    expect(save).toBeDisabled();
  });

  it.each(['document', 'reader'] as const)(`keeps an old pending act separate after %s replacement (${locale})`, async (change) => {
    await setLocale(locale);
    const write = deferred<SummaryOptionsReceipt>();
    source.write.mockReturnValueOnce(write.promise);
    const close = vi.fn();
    const page = show(<DocumentSummaryOptionsDialog documentId="document-a" onOpenChange={close} />);
    await screen.findByText(i18n.t('summaryOptions.document.inherited'));
    await userEvent.click(screen.getByRole('switch', { name: i18n.t('summaryOptions.fields.automatic') }));
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.save') }));
    if (change === 'document') page.rerender(<DocumentSummaryOptionsDialog documentId="document-b" onOpenChange={close} />);
    else act(() => { client.session.setState({ userId: 'summary-reader-b', readerVersion: 41 }); });
    await screen.findByText(i18n.t('summaryOptions.document.inherited'));
    await act(async () => { write.resolve(receipt('document-a', 1)); await write.promise; });
    expect(close).not.toHaveBeenCalled();
    expect(screen.getByRole('switch', { name: i18n.t('summaryOptions.fields.automatic') })).not.toBeChecked();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: i18n.t('common.save') })).toBeDisabled();
  });
}
