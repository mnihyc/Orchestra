/** A package export starts with the contents needed for a round trip and keeps each explicit choice. */
import { QueryClient } from '@tanstack/react-query';
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState, type ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Exports from '@/api/exports';
import { PlaceOptions } from '@/features/export/PlaceOptions';
import { PlanLine } from '@/features/export/PlanLine';
import { readPlan } from '@/features/export/plan';
import { YourExports } from '@/features/export/YourExports';
import { saveFile } from '@/features/export/save';
import { OPENING_CHOICES, type Choices } from '@/features/export/choices';
import type { Export as ExportRow } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { show } from './support';

const archives = vi.hoisted(() => ({ items: [] as ExportRow[] }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
vi.mock('@/features/settings/PersonalFrame', () => ({ PersonalFrame: ({ children }: { children: ReactNode }) => <section>{children}</section> }));
vi.mock('@/features/export/save', () => ({ saveFile: vi.fn() }));
vi.mock('@/api/exports', async (load) => ({ ...await load<typeof Exports>(), myExportsQuery: {
  queryKey: ['export-list-fixture'], initialPageParam: null, queryFn: () => Promise.resolve({ items: archives.items, next_cursor: null }), getNextPageParam: () => null,
} }));
const clients: QueryClient[] = [];
const appearance = useAppearance.getState();
afterEach(async () => { archives.items = []; clients.splice(0).forEach((client) => client.clear()); vi.mocked(saveFile).mockClear(); useAppearance.setState(appearance); await setLocale('en'); });

function Options() {
  const [choices, setChoices] = useState<Choices>(OPENING_CHOICES);
  return <PlaceOptions choices={choices} onChange={(patch) => setChoices((current) => ({ ...current, ...patch }))} />;
}

it.each(['en', 'zh-CN'] as const)('offers a package and allows its files and comments to be switched off in %s', async (locale) => {
  try {
    await setLocale(locale);
    const user = userEvent.setup();
    show(<Options />);
    const packageChoice = screen.getByRole('radio', { name: i18n.t('choiceCatalog.exportFormat.package.name') });
    await user.click(packageChoice);
    expect(packageChoice).toBeChecked();
    expect(screen.getByText(i18n.t('importWizard.export.packageNote'))).toBeVisible();
    const files = screen.getByRole('checkbox', { name: i18n.t('export.more.files') });
    const comments = screen.getByRole('checkbox', { name: i18n.t('export.more.comments') });
    expect(files).toBeChecked();
    expect(comments).toBeChecked();
    await user.click(files);
    await user.click(comments);
    expect(files).not.toBeChecked();
    expect(comments).not.toBeChecked();
  } finally { await setLocale('en'); }
});

for (const locale of ['en', 'zh-CN'] as const) {
  it(`shows the effective server file allowance beside the archive allowance in ${locale}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale });
    const plan = { node: { id: 'project', kind: 'project' as const, name: 'Work', redacted: false }, counts: { places: 1, conversations: 1, chats: 0, documents: 0, files: 1 },
      bytes: 128, limits: { max_bytes: 2_097_152, max_file_bytes: 524_288, max_items: 10, keep_hours: 24 }, left_out: [], blockers: [] };
    show(<PlanLine reading={readPlan(plan)} counting={false} error={null} onRetry={vi.fn()} idle={null} />);
    const file = formatBytes(plan.limits.max_file_bytes, locale);
    expect(screen.getByRole('status')).toHaveTextContent(locale === 'en' ? `Files up to ${file} · server` : `单文件最多 ${file} · 服务器`);
    expect(screen.getByRole('status')).toHaveTextContent(formatBytes(plan.limits.max_bytes, locale));
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`names a partial archive beside its download and saves its actual omission report in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme });
    const manifest = { of: { name: 'Work' }, counts: { files: 1 }, left_out: [{ what: 'too_large', count: 2, where: null }] };
    archives.items = [{ id: 'archive', node: { id: 'project', name: 'Work', kind: 'project', redacted: false }, request: { what: 'all', files: true }, state: 'ready', job_id: 'job',
      manifest, bytes: 128, counts: { places: 1, conversations: 1, chats: 0, documents: 0, files: 1 }, left_out: manifest.left_out.map((row) => ({ ...row, what: 'too_large' as const })),
      created_at: '2026-10-01T00:00:00Z', ready_at: '2026-10-01T00:01:00Z', expires_at: '2030-01-01T00:00:00Z', downloaded_at: null }];
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(client);
    show(<YourExports />, client);
    const table = await screen.findByRole('table', { name: i18n.t('export.page.title') });
    expect(within(table).getByRole('button', { name: locale === 'en' ? 'Download partial export of Work' : '下载 Work 的部分导出' })).toBeVisible();
    expect(table).toHaveTextContent(locale === 'en' ? 'Partial · report included' : '部分导出 · 内含报告');
    const user = userEvent.setup();
    await user.click(within(table).getByRole('button', { name: locale === 'en' ? 'Download omission report for Work' : '下载 Work 的省略报告' }));
    const saved = vi.mocked(saveFile).mock.calls.at(-1);
    expect(saved?.[1]).toBe('export-report.json');
    if (saved === undefined) throw new Error('The report is handed to the browser.');
    const text = await new Promise<string>((resolve, reject) => { const reader = new FileReader(); reader.onerror = () => reject(reader.error ?? new Error('The report could not be read.')); reader.onload = () => typeof reader.result === 'string' ? resolve(reader.result) : reject(new Error('The report is text.')); reader.readAsText(saved[0]); });
    const report: unknown = JSON.parse(text); expect(report).toEqual(manifest);
  });
}

it('keeps an intentionally narrowed archive ready without calling it a partial write', async () => {
  archives.items = [{ id: 'complete', node: null, request: { what: 'all', files: false }, state: 'ready', job_id: null, manifest: { left_out: [] }, bytes: 128,
    counts: { places: 1, conversations: 1, chats: 0, documents: 0, files: 0 }, left_out: [], created_at: '2026-10-01T00:00:00Z', ready_at: '2026-10-01T00:01:00Z', expires_at: null, downloaded_at: null }];
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } }); clients.push(client); show(<YourExports />, client);
  const table = await screen.findByRole('table', { name: i18n.t('export.page.title') });
  expect(table).toHaveTextContent(i18n.t('export.list.ready'));
  expect(table).not.toHaveTextContent('Partial');
});
