/** Real generated reply shapes feed the shared strip; actions retain reader, native target and root. */
import type * as Summaries from '@/api/summaries';
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { queryOptions } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import type { SummarySnapshot as Reply } from '@/generated/api-types';
import { ResourceSummary } from '@/features/summaries/ResourceSummary';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const source = vi.hoisted(() => ({ reply: undefined as Reply | undefined, readFailures: 0, refresh: vi.fn(), ask: vi.fn() }));
vi.mock('@/features/ask/open', () => ({ openAskWithDraft: source.ask }));
vi.mock('@/api/summaries', async (original) => ({ ...await original<typeof Summaries>(),
  resourceSummaryQuery: (target: { kind: string; id: string }) => queryOptions({ queryKey: ['native-summary-case', target.kind, target.id], queryFn: () => {
    if (source.readFailures > 0) { source.readFailures -= 1; return Promise.reject(new Error('The free readback failed')); }
    return Promise.resolve(source.reply);
  }, staleTime: 0, retry: false }),
  refreshResourceSummary: source.refresh,
}));
const initial = (): Reply => ({ root_org_id: 'root-b', target: { kind: 'document', id: 'doc' }, inspecting: false, revision: 1, state: 'fresh', text: 'A readable source paragraph.',
  producer: { kind: 'model', title: 'Hidden supplier canary', provider: null, offering: null }, updated_at: '2026-11-08T10:00:00Z', route: null, input: null, manual_problem: null, wait: null, poll_after_ms: null, problem: null });
afterEach(async () => { cleanup(); source.readFailures = 0; source.ask.mockClear(); source.refresh.mockReset(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows the actual scheduled time and reason while retaining the paragraph (${locale})`, async () => {
    await setLocale(locale); source.reply = { ...initial(), state: 'scheduled', wait: { reason: 'interval', until: '2026-11-09T02:00:00Z' }, poll_after_ms: 900_000 };
    const page = show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    expect(await screen.findByText(i18n.t('summaryBox.state.scheduled'))).toBeVisible();
    expect(screen.getByText(i18n.t('summaryBox.wait.interval'))).toBeVisible();
    expect(page.container.querySelector('time[datetime="2026-11-09T02:00:00Z"]')).not.toBeNull();
    expect(screen.getByText('A readable source paragraph.')).toBeVisible();
    expect(source.refresh).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') })).not.toHaveAttribute('aria-disabled', 'true');
  });
  it(`offers a personal default refresh without an automatic producer or configuration action (${locale})`, async () => {
    await setLocale(locale); source.reply = { ...initial(), state: 'empty', text: null, producer: null, manual_problem: null };
    source.refresh.mockImplementation(() => { source.reply = { ...initial(), revision: 2, text: 'A personal default summary.' }; return Promise.resolve(); });
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await userEvent.click(await screen.findByRole('button', { name: i18n.t('summaryBox.refresh') }));
    await screen.findByText('A personal default summary.');
    expect(source.refresh).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole('button', { name: i18n.t('summaryBox.choose') })).not.toBeInTheDocument();
  });
  it.each(['no_model', 'cap_reached'] as const)(`explains %s without a paid attempt (${locale})`, async (problem) => {
    await setLocale(locale); source.reply = { ...initial(), state: 'no_summarizer', text: null, producer: null, manual_problem: problem };
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    expect(await screen.findByText(i18n.t(problem === 'cap_reached' ? 'summaryBox.manualCap' : 'summaryBox.manualModel'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('summaryBox.refresh') })).not.toBeInTheDocument();
    expect(source.refresh).not.toHaveBeenCalled();
  });
  it(`shows actual stored input coverage without another refresh or Ask (${locale})`, async () => {
    await setLocale(locale); source.reply = { ...initial(), input: { included_items: 5, shortened_items: 3, omitted_items: 12 } };
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.details') }));
    expect(await screen.findByText(i18n.t('summaryBox.inputFacts', { included: 5, shortened: 3, omitted: 12 }))).toBeVisible();
    expect(source.refresh).not.toHaveBeenCalled(); expect(source.ask).not.toHaveBeenCalled();
  });
  it(`does not invent zero coverage when the stored source supplies no input facts (${locale})`, async () => {
    await setLocale(locale); source.reply = initial();
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.details') }));
    expect(screen.queryByText(i18n.t('summaryBox.inputFacts', { included: 0, shortened: 0, omitted: 0 }))).not.toBeInTheDocument();
    expect(source.refresh).not.toHaveBeenCalled();
  });
  it(`keeps passive inspection readable without a new model or Ask act (${locale})`, async () => {
    await setLocale(locale); source.reply = { ...initial(), inspecting: true };
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    const ask = screen.getByRole('button', { name: i18n.t('summaryBox.ask') });
    const refresh = screen.getByRole('button', { name: i18n.t('summaryBox.refresh') });
    expect(ask).toHaveAttribute('aria-disabled', 'true');
    expect(refresh).toHaveAttribute('aria-disabled', 'true');
    await userEvent.click(ask); await userEvent.click(refresh);
    expect(source.ask).not.toHaveBeenCalled(); expect(source.refresh).not.toHaveBeenCalled();
  });
  it(`opens a quoted editable draft in the actual source root without sending (${locale})`, async () => {
    await setLocale(locale); source.reply = initial();
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    expect(screen.getByText(i18n.t('summaryBox.model'))).toBeVisible();
    expect(screen.queryByText('Hidden supplier canary')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.ask') }));
    expect(source.ask).toHaveBeenCalledExactlyOnceWith({ root: 'root-b', text: `${i18n.t('summaryBox.ask')}\n\n/doc/doc\n\n> A readable source paragraph.` });
    expect(source.refresh).not.toHaveBeenCalled();
  });
  it(`refreshes only its admitted target then reads native state (${locale})`, async () => {
    await setLocale(locale); source.reply = initial();
    source.refresh.mockImplementation(() => { source.reply = { ...initial(), revision: 2, state: 'updating' }; return Promise.resolve(); });
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
    await waitFor(() => expect(source.refresh).toHaveBeenCalledTimes(1));
    expect(source.refresh.mock.calls[0]?.[0]).toEqual({ kind: 'document', id: 'doc' });
    expect(source.refresh.mock.calls[0]?.[1]).toEqual(expect.any(String));
    await screen.findByText(i18n.t('summaryBox.state.updating'));
    expect(screen.getByText('A readable source paragraph.')).toBeVisible();
  });
  it(`clears removed projected text even when the native cache revision did not change (${locale})`, async () => {
    await setLocale(locale); source.reply = initial();
    source.refresh.mockImplementation(() => { source.reply = { ...initial(), text: null, producer: null, updated_at: null, state: 'stale' }; return Promise.resolve(); });
    show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
    await screen.findByText('A readable source paragraph.');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
    await waitFor(() => expect(screen.queryByText('A readable source paragraph.')).not.toBeInTheDocument());
  });
}

for (const locale of ['en', 'zh-CN'] as const) it(`reuses an unresolved paid refresh identity and allocates a new one after acknowledgement (${locale})`, async () => {
  await setLocale(locale); source.reply = initial();
  source.refresh.mockRejectedValueOnce(new Error('The accepted refresh acknowledgement was lost'))
    .mockImplementationOnce(() => { source.reply = { ...initial(), revision: 2, text: 'The acknowledged updated paragraph.' }; return Promise.resolve(); })
    .mockResolvedValue(undefined);
  show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
  await screen.findByText('A readable source paragraph.');
  const refresh = () => screen.getByRole('button', { name: i18n.t('summaryBox.refresh') });
  await userEvent.click(refresh()); await screen.findByRole('alert');
  await userEvent.click(refresh());
  await screen.findByText('The acknowledged updated paragraph.');
  expect(source.refresh).toHaveBeenCalledTimes(2);
  expect(source.refresh.mock.calls[1]?.[1]).toBe(source.refresh.mock.calls[0]?.[1]);
  await userEvent.click(refresh());
  await waitFor(() => expect(source.refresh).toHaveBeenCalledTimes(3));
  expect(source.refresh.mock.calls[2]?.[1]).not.toBe(source.refresh.mock.calls[0]?.[1]);
});

for (const locale of ['en', 'zh-CN'] as const) it(`keeps a pending refresh when the same target is rendered with another property order (${locale})`, async () => {
  await setLocale(locale); source.reply = initial();
  source.refresh.mockRejectedValueOnce(new Error('Acknowledgement lost')).mockResolvedValue(undefined);
  const page = show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
  await screen.findByText('A readable source paragraph.');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
  await screen.findByRole('alert');
  page.rerender(<ResourceSummary target={{ id: 'doc', kind: 'document' }} reference="/doc/doc" />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
  await waitFor(() => expect(source.refresh).toHaveBeenCalledTimes(2));
  expect(source.refresh.mock.calls[1]?.[1]).toBe(source.refresh.mock.calls[0]?.[1]);
});

for (const locale of ['en', 'zh-CN'] as const) it(`retries only the free read after an acknowledged paid refresh (${locale})`, async () => {
  await setLocale(locale); source.reply = initial();
  source.refresh.mockImplementationOnce(() => {
    source.reply = { ...initial(), revision: 2, text: 'The accepted refresh is readable after retry.' };
    source.readFailures = 1;
    return Promise.resolve();
  }).mockResolvedValue(undefined);
  show(<ResourceSummary target={{ kind: 'document', id: 'doc' }} reference="/doc/doc" />);
  await screen.findByText('A readable source paragraph.');
  await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
  await screen.findByRole('alert');
  expect(source.refresh).toHaveBeenCalledTimes(1);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  await screen.findByText('The accepted refresh is readable after retry.');
  expect(source.refresh).toHaveBeenCalledTimes(1);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('summaryBox.refresh') }));
  await waitFor(() => expect(source.refresh).toHaveBeenCalledTimes(2));
  expect(source.refresh.mock.calls[1]?.[1]).not.toBe(source.refresh.mock.calls[0]?.[1]);
});
