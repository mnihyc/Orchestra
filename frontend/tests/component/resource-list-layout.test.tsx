/** D438-8: all resource kinds expose the same selection, identity and visible main action. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Box } from 'lucide-react';
import { afterEach, expect, it, vi } from 'vitest';

import { ResourceListFrame } from '@/features/resources/ResourceList';
import { entriesOf, RowMenuCell, RowPrimaryCell } from '@/features/resources/acts';
import type { ResourceKind } from '@/features/resources/trail';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { Button } from '@/ui/Button';
import { show } from './support';

vi.mock('@/features/resources/trail', () => ({ useResourceHeading: vi.fn(), rememberList: vi.fn() }));
vi.mock('@/features/resources/ResourceNav', () => ({ ResourceNav: () => null }));

const kinds: readonly ResourceKind[] = ['providers', 'agents', 'machines', 'environments', 'templates', 'hosts', 'mcp', 'mounts'];
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const appearance = useAppearance.getState();
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

it('keeps the visible main act out of its row menu while preserving the other action', async () => {
  const user = userEvent.setup(); const open = vi.fn(); const copy = vi.fn();
  const primary = { id: 'open', label: i18n.t('resources.act.open'), run: open };
  const secondary = { id: 'copy-link', label: i18n.t('resources.act.copyLink'), run: copy };
  const acts = { primary, entries: entriesOf([primary], [secondary]), keys: {}, busy: false };
  show(<><RowPrimaryCell name="Build tools" acts={acts} /><RowMenuCell name="Build tools" acts={acts} working={false} /></>);
  await user.click(screen.getByRole('button', { name: i18n.t('resources.act.menuOf', { name: 'Build tools' }) }));
  const menu = screen.getByRole('menu');
  expect(within(menu).queryByRole('menuitem', { name: primary.label })).not.toBeInTheDocument();
  expect(within(menu).queryByRole('separator')).not.toBeInTheDocument();
  await user.click(within(menu).getByRole('menuitem', { name: secondary.label }));
  expect(copy).toHaveBeenCalledTimes(1); expect(open).not.toHaveBeenCalled();
});

// Draws every row and types through it as a person would: 4 to 15 s on a busy machine, so 30 s as the suite's other long cases.
for (const [locale, theme] of modes) it(`keeps the eight list layouts and reader selection alike in ${locale}/${theme}`, { timeout: 30_000 }, async () => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  const user = userEvent.setup();
  for (const kind of kinds) {
    const open = vi.fn(); const create = vi.fn(); const row = { id: 'one', name: 'Build tools', owner: null };
    const primary = { id: 'open', label: i18n.t('resources.act.open'), run: open };
    const view = show(<ResourceListFrame kind={kind} search={{}} title={i18n.t(`screens.${kind}.title`)} icon={Box} lead={null} label={i18n.t(`screens.${kind}.title`)}
      load={{ isPending: false, isError: false, error: null, isRefetching: false, refetch: vi.fn() }} all={[row]} rows={[row]}
      columns={[{ id: 'name', header: i18n.t('resources.listName'), cell: (value) => <a href={`#${value.id}`}>{value.name}</a> }]}
      rowKey={(value) => value.id} text="" onText={vi.fn()} searchLabel={i18n.t('resources.inventorySearch')} searchPlaceholder=""
      action={<Button onClick={create}>{i18n.t('common.add')}</Button>} empty={{ icon: <Box />, title: '' }} noMatch=""
      acts={{ name: (value) => value.name, of: () => ({ primary, busy: false, entries: [], keys: {} }) }} />);
    const table = screen.getByRole('table');
    expect(table.querySelector('th:first-child')).toHaveAttribute('data-table-column', 'select');
    expect(table.querySelector('td[data-table-column="name"]')).toHaveTextContent(row.name);
    const act = within(table).getByRole('button', { name: i18n.t('resources.act.of', { act: primary.label, name: row.name }) });
    expect(act).toBeVisible(); await user.click(act); expect(open).toHaveBeenCalledTimes(1);
    const add = screen.getByRole('button', { name: i18n.t('common.add') });
    expect(add.closest('[data-resource-kind]')).toHaveAttribute('data-resource-kind', kind);
    expect(view.container.querySelector('[data-filter-bar]')).not.toContainElement(add);
    await user.click(add); expect(create).toHaveBeenCalledTimes(1);
    await user.click(within(table).getByRole('checkbox', { name: i18n.t('resources.bulk.select', { name: row.name }) }));
    const selection = screen.getByRole('toolbar', { name: i18n.t('resources.bulk.label') });
    expect(selection).toHaveTextContent(i18n.t('resources.bulk.count', { count: 1 }));
    // Selecting readable rows grants no management action.
    expect(within(selection).getAllByRole('button')).toHaveLength(1);
    await user.click(within(selection).getByRole('button', { name: i18n.t('resources.bulk.clear') }));
    expect(screen.queryByRole('toolbar', { name: i18n.t('resources.bulk.label') })).not.toBeInTheDocument();
    view.unmount();
  }
});
