/** Management uses the real paged resource lists, at system scope and inside its existing frame. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { AnchorHTMLAttributes, ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import type * as Inventory from '@/api/everything';
import type { EverythingFilter } from '@/api/everything';
import type { EverythingRowPage } from '@/generated/api-types';
import { Templates } from '@/features/admin/Templates';
import { Mcp } from '@/features/admin/Mcp';
import { i18n, setLocale } from '@/i18n';
import { me } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ list: vi.fn<() => Promise<EverythingRowPage>>(), filter: vi.fn<(filter: EverythingFilter) => void>(), heading: vi.fn<(...args: unknown[]) => undefined>(), nav: vi.fn<() => void>() }));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(), meQuery: { queryKey: ['me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada')) } }));
vi.mock('@/api/everything', async (load) => ({ ...await load<typeof Inventory>(),
  everythingQuery: (reader: string, parent: string | null, filter: EverythingFilter, root: string) => {
    calls.filter(filter);
    return { queryKey: [root, reader, parent, filter], initialPageParam: null as string | null, queryFn: calls.list,
      getNextPageParam: (page: EverythingRowPage) => page.next_cursor };
  },
}));
vi.mock('@/features/resources/trail', async (load) => ({ ...await load<object>(), useResourceHeading: (...args: unknown[]) => calls.heading(...args), rememberList: vi.fn() }));
vi.mock('@/features/resources/ResourceNav', () => ({ ResourceNav: () => { calls.nav(); return null; } }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => vi.fn(),
  Link: ({ to, children, params: _params, ...props }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; params?: unknown; children?: ReactNode }) => <a {...props} href={to}>{children}</a>,
}));

beforeEach(() => { vi.clearAllMocks(); calls.list.mockResolvedValue({ items: [], next_cursor: null }); });
afterEach(async () => { cleanup(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`loads both template kinds through the native system pager without repeating the Management heading in ${locale}`, async () => {
    await setLocale(locale);
    show(<Templates />);
    await screen.findByRole('button', { name: i18n.t('templates.list.add') });
    await waitFor(() => expect(calls.list).toHaveBeenCalledOnce());
    const first = calls.filter.mock.calls.at(-1)?.[0];
    expect(first).toMatchObject({ scope: 'system', kinds: ['template'], resource: { details: true } });
    expect(first?.resource?.kind).toBeUndefined();
    expect(calls.heading).toHaveBeenLastCalledWith('templates', expect.anything(), undefined, undefined, false);
    expect(calls.nav).not.toHaveBeenCalled();
    expect(screen.queryByRole('heading', { name: i18n.t('screens.templates.title') })).not.toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: i18n.t('templates.list.add') })).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('resources.controls.filter') }));
    const provider = screen.getByRole('radio', { name: i18n.t('templates.purposeChoices.provider.name') });
    await userEvent.click(provider);
    await waitFor(() => expect(calls.filter).toHaveBeenLastCalledWith(expect.objectContaining({ scope: 'system', resource: { details: true, kind: 'provider', view: 'usable' } })));
  });

  it(`opens the working MCP preset list at system scope with its ordinary creation act in ${locale}`, async () => {
    await setLocale(locale);
    show(<Mcp />);
    await screen.findByRole('button', { name: i18n.t('resourceDetails.mcp.add') });
    await waitFor(() => expect(calls.list).toHaveBeenCalledOnce());
    expect(calls.filter).toHaveBeenLastCalledWith(expect.objectContaining({ scope: 'system', kinds: ['mcp_server'], resource: { details: true, view: 'usable' } }));
    expect(calls.heading).toHaveBeenLastCalledWith('mcp', expect.anything(), undefined, undefined, false);
    expect(calls.nav).not.toHaveBeenCalled();
    expect(screen.queryByRole('heading', { name: i18n.t('screens.mcp.title') })).not.toBeInTheDocument();
    expect(screen.getAllByRole('button', { name: i18n.t('resourceDetails.mcp.add') })).toHaveLength(1);
  });
}
