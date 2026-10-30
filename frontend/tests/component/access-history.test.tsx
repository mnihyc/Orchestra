/** The existing history reader works for every resource, only when opened. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ClientApi from '@/api/client';
import type { AccessKind } from '@/api/access';
import { ByArea } from '@/features/access/ByArea';
import type { Changes } from '@/features/access/changes';
import type { HistoryLinePage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { CATALOGUE, pageFor } from '../unit/general-access-fixtures';
import { show } from './support';

interface ReadOptions { params: { path: { object: string; id: string }; query: { cursor?: string; depth: string } } }
const calls = vi.hoisted(() => ({ read: vi.fn<(path: string, options: ReadOptions) => Promise<{ data: HistoryLinePage; response: Response }>>() }));
vi.mock('@/api/client', async (load) => ({ ...await load<typeof ClientApi>(), api: { GET: calls.read } }));
// The access-grid controls have their own suites; the real disclosure, API query and history stay mounted.
vi.mock('@/features/access/RowCell', () => ({ RowCell: () => null }));
const changes: Changes = { busy: false, doing: null, error: null, stale: null, forget: vi.fn(), again: () => Promise.resolve(false), run: () => Promise.resolve(true) };
const previous = useAppearance.getState(), oldTheme = document.documentElement.dataset.theme, oldDark = document.documentElement.classList.contains('dark');
const kinds = [['provider', 'providers'], ['agent', 'agents'], ['environment', 'environments'], ['host', 'hosts'], ['machine', 'machines'], ['template', 'templates'], ['mount', 'mounts'], ['mcp_server', 'mcp-servers']] as const satisfies readonly (readonly [AccessKind, string])[];
const line = (id: string, text: string) => ({ id, text, actor: null, action: 'resource.updated', at: '2026-10-16T02:30:00Z', about: null });
beforeEach(() => { vi.clearAllMocks(); calls.read.mockReset(); });
afterEach(async () => {
  cleanup(); useAppearance.setState(previous); document.documentElement.classList.toggle('dark', oldDark);
  if (oldTheme === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = oldTheme;
  await setLocale('en');
});

function history(kind: AccessKind) {
  const page = pageFor(kind), target = { kind, id: page.object.id, name: 'Research' };
  show(<ByArea target={target} page={page} catalogue={CATALOGUE} source={{ type: 'page', target }} changes={changes} ask={vi.fn()} onBack={vi.fn()} />);
  return target;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const [kind, path] of kinds) {
  it(`reads and pages ${kind} history without a hidden-view request (${locale}/${theme})`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
    calls.read.mockResolvedValueOnce({ data: { items: [line('first', 'A name was changed.')], next_cursor: 'next-page' }, response: new Response(null, { status: 200 }) })
      .mockResolvedValueOnce({ data: { items: [line('second', 'Access was changed.')], next_cursor: null }, response: new Response(null, { status: 200 }) });
    const user = userEvent.setup(), target = history(kind);
    expect(calls.read).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: i18n.t('access.history.title') }));
    const list = await screen.findByRole('list', { name: i18n.t('access.history.label') });
    expect(within(list).getByText('A name was changed.')).toBeVisible();
    expect(calls.read).toHaveBeenCalledTimes(1);
    const first = calls.read.mock.calls[0];
    expect(first?.[0]).toBe('/{object}/{id}/history');
    expect(first?.[1].params.path).toEqual({ object: path, id: target.id });
    expect(first?.[1].params.query.depth).toBe('every');
    expect(first?.[1].params.query.cursor).toBeUndefined();
    await user.click(screen.getByRole('button', { name: i18n.t('ui.pagination.more') }));
    expect(await within(list).findByText('Access was changed.')).toBeVisible();
    expect(within(list).getAllByRole('listitem')).toHaveLength(2);
    expect(calls.read).toHaveBeenCalledTimes(2);
    const second = calls.read.mock.calls[1];
    expect(second?.[0]).toBe('/{object}/{id}/history');
    expect(second?.[1].params.path).toEqual({ object: path, id: target.id });
    expect(second?.[1].params.query.depth).toBe('every');
    expect(second?.[1].params.query.cursor).toBe('next-page');
    expect(screen.queryByRole('button', { name: i18n.t('ui.pagination.more') })).not.toBeInTheDocument();
  });
}

it.each(['en', 'zh-CN'] as const)('shows a resource read failure and retries the same reader (%s)', async (locale) => {
  await setLocale(locale);
  calls.read.mockRejectedValueOnce(new Error('history connection failed')).mockResolvedValueOnce({ data: { items: [], next_cursor: null }, response: new Response(null, { status: 200 }) });
  const user = userEvent.setup(); history('provider');
  await user.click(screen.getByRole('button', { name: i18n.t('access.history.title') }));
  await user.click(await screen.findByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(await screen.findByText(i18n.t('access.history.none'))).toBeVisible();
  await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(2));
});
