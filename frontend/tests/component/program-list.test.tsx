/** Program summaries retain supplied names, platform builds and status in narrow and wide panes. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ProgramList, type ProgramListEntry } from '@/features/admin/ProgramList';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { vi.restoreAllMocks(); vi.unstubAllGlobals(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`opens a named third program and keeps every reported summary in a narrow pane (${locale})`, async () => {
  await setLocale(locale);
  vi.stubGlobal('ResizeObserver', class { observe = vi.fn(); disconnect = vi.fn(); unobserve = vi.fn(); });
  const name = locale === 'en' ? 'Research assistant' : '研究助理';
  const row: ProgramListEntry = { kind: 'research-agent', name, protocol: 'ACP', shipped: [{ version: '1.0', platform: 'Linux · x86_64' }],
    newestChecked: [{ version: '1.2', platform: 'Linux · x86_64' }], channel: { state: 'idle', label: locale === 'en' ? 'Scheduled' : '已计划' },
    check: { state: 'done', label: i18n.t('admin.harnesses.status.checked') } };
  const open = vi.fn();
  show(<ProgramList programs={[row]} onOpen={open} />);
  const table = screen.getByRole('table', { name: i18n.t('programList.title') });
  const program = within(table).getByRole('row', { name: new RegExp(name) });
  expect(program).toHaveTextContent('research-agent');
  expect(program).toHaveTextContent('ACP');
  expect(program).toHaveTextContent('1.0 · Linux · x86_64');
  expect(program).toHaveTextContent('1.2 · Linux · x86_64');
  expect(program).toHaveTextContent(row.channel.label);
  expect(program).toHaveTextContent(row.check.label);
  const link = within(program).getByRole('link', { name });
  expect(link).toHaveAttribute('href', '/admin/harnesses?program=research-agent');
  await userEvent.click(link);
  expect(open).toHaveBeenCalledExactlyOnceWith('research-agent');
});

it('shows the existing empty view when no programs are declared', () => {
  vi.stubGlobal('ResizeObserver', class { observe = vi.fn(); disconnect = vi.fn(); unobserve = vi.fn(); });
  show(<ProgramList programs={[]} onOpen={vi.fn()} />);
  expect(screen.getByText(i18n.t('programList.empty'))).toBeVisible();
  expect(screen.queryByRole('table')).not.toBeInTheDocument();
});

it('gives channel and check their own columns when the pane is wide', () => {
  vi.stubGlobal('ResizeObserver', class { observe = vi.fn(); disconnect = vi.fn(); unobserve = vi.fn(); });
  vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(1200);
  const program: ProgramListEntry = { kind: 'research-agent', name: 'Research assistant', protocol: 'ACP', shipped: [], newestChecked: [],
    channel: { state: 'idle', label: 'Scheduled' }, check: { state: 'done', label: 'Checked' } };
  show(<ProgramList programs={[program]} onOpen={vi.fn()} />);
  expect(screen.getByRole('columnheader', { name: i18n.t('programList.channel') })).toBeVisible();
  expect(screen.getByRole('columnheader', { name: i18n.t('programList.check') })).toBeVisible();
  expect(screen.queryByRole('columnheader', { name: i18n.t('programList.status') })).not.toBeInTheDocument();
});
