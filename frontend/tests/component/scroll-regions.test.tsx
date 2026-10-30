/** Scrolling content is reached and named, while long read-only boxes expand with the page. */
import { screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { CodeBlock } from '@/ui/CodeBlock';
import { DiffView } from '@/ui/DiffView';
import { JsonTree } from '@/ui/JsonTree';
import { Table } from '@/ui/Table';
import { show } from './support';

beforeEach(async () => {
  await setLocale('en');
  class Observer { observe = vi.fn(); unobserve = vi.fn(); disconnect = vi.fn(); }
  vi.stubGlobal('ResizeObserver', Observer);
});
afterEach(async () => { vi.unstubAllGlobals(); await setLocale('en'); });

it('puts a named table region between the preceding and following keyboard stops without losing its data', async () => {
  const user = userEvent.setup();
  show(<><button>Before table</button><Table label="Adapter support" rows={[{ id: 'one', name: 'Adapter' }]}
    rowKey={(row) => row.id} columns={[{ id: 'name', header: 'Name', cell: (row) => row.name }]} /><button>After table</button></>);
  await user.tab();
  expect(screen.getByRole('button', { name: 'Before table' })).toHaveFocus();
  await user.tab();
  const region = screen.getByRole('region', { name: 'Adapter support' });
  expect(region).toHaveFocus();
  expect(within(region).getByRole('table', { name: 'Adapter support' })).toBeVisible();
  expect(within(region).getByRole('cell', { name: 'Adapter' })).toBeVisible();
  await user.tab();
  expect(screen.getByRole('button', { name: 'After table' })).toHaveFocus();
});

it('gives a difference a named keyboard region and preserves its changed lines', async () => {
  const user = userEvent.setup();
  show(<DiffView label="Changes to auth.ts" diff={'@@ -1 +1 @@\n-oldValue\n+newValue\n'} />);
  const region = screen.getByRole('region', { name: 'Changes to auth.ts' });
  expect(region).toHaveAttribute('tabindex', '0');
  expect(within(region).getByText('oldValue')).toBeVisible();
  expect(within(region).getByText('newValue')).toBeVisible();
  screen.getByRole('button', { name: 'Copy patch' }).focus();
  await user.tab();
  expect(region).toHaveFocus();
});

it.each(['en', 'zh-CN'] as const)('previews long code, copies all of it and expands without a height-limited box in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue();
  const code = Array.from({ length: 12 }, (_, index) => `line ${index + 1}`).join('\n');
  show(<CodeBlock code={code} lang="text" maxHeight="24rem" />);
  const region = screen.getByRole('region', { name: i18n.t('ui.code.label', { language: 'text' }) });
  expect(region).toHaveAttribute('tabindex', '0');
  expect(region).not.toHaveTextContent('line 12');
  expect(region.style.maxHeight).toBe('');
  await user.click(screen.getByRole('button', { name: i18n.t('ui.code.copy') }));
  expect(copy).toHaveBeenLastCalledWith(code);
  const more = screen.getByRole('button', { name: i18n.t('ui.preview.showAll') });
  more.focus();
  await user.keyboard('{Enter}');
  expect(region).toHaveTextContent('line 12');
  expect(region.style.maxHeight).toBe('');
  await user.click(screen.getByRole('button', { name: i18n.t('ui.preview.showLess') }));
  expect(region).not.toHaveTextContent('line 12');
});

it.each(['en', 'zh-CN'] as const)('names JSON branch toggles by their path and state in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<JsonTree label="Configuration" value={{ limits: { machine: { cores: 4 } } }} />);
  const open = screen.getByRole('button', { name: i18n.t('ui.json.expand', { name: 'Configuration.limits.machine' }) });
  expect(open).toHaveAttribute('aria-expanded', 'false');
  expect(screen.queryByText('cores:')).not.toBeInTheDocument();
  open.focus();
  await user.keyboard('{Enter}');
  expect(screen.getByRole('button', { name: i18n.t('ui.json.collapse', { name: 'Configuration.limits.machine' }) })).toHaveFocus();
  expect(screen.getByText('cores:')).toBeVisible();
  await user.keyboard(' ');
  expect(open).toHaveAttribute('aria-expanded', 'false');
  expect(screen.queryByText('cores:')).not.toBeInTheDocument();
});

it('expands a long descriptor in the page and puts hidden JSON controls outside the initial tab order', async () => {
  const user = userEvent.setup();
  const value = Object.fromEntries(Array.from({ length: 12 }, (_, index) => [`field${index}`, index]));
  show(<JsonTree preview label="Adapter descriptor" value={value} />);
  expect(screen.getByRole('region', { name: 'Adapter descriptor' })).toHaveAttribute('tabindex', '0');
  expect(screen.queryByRole('button', { name: 'Collapse Adapter descriptor' })).not.toBeInTheDocument();
  expect(screen.queryByText('field11:')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Show all' }));
  expect(screen.getByRole('button', { name: 'Collapse Adapter descriptor' })).toBeVisible();
  expect(screen.getByText('field11:')).toBeVisible();
  await user.click(screen.getByRole('button', { name: 'Show less' }));
  expect(screen.queryByText('field11:')).not.toBeInTheDocument();
});
