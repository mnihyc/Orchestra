/** Share names the holder, its level and its source; people behind a group load only on request. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Access from '@/api/access';
import { AccessRoster } from '@/features/access/AccessRoster';
import type { Changes } from '@/features/access/changes';
import { linesOf } from '@/features/access/model';
import type { Group, PermissionPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { ADA, BEA, CATALOGUE, named, pageFor, TOPIC } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ people: vi.fn(), take: vi.fn(), level: vi.fn() }));
vi.mock('@/api/access', async (original) => {
  const api = await original<typeof Access>();
  return { ...api, peopleQuery: (...args: Parameters<typeof Access.peopleQuery>) => ({ ...api.peopleQuery(...args), queryFn: () => {
    calls.people(...args); return Promise.resolve({ items: [BEA], next_cursor: null });
  } }) };
});
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');
const changes: Changes = { busy: false, doing: null, error: null, stale: null, forget: () => undefined, again: () => Promise.resolve(false), run: () => Promise.resolve(false) };
const crew: Group = { id: 'crew', org_id: 'acme', name: 'Crew', description: '', member_count: 1, place: 'Acme' };
const target = { kind: 'thread' as const, id: 'thread', name: 'Example', orgId: 'acme' };
function page(): PermissionPage {
  const base = pageFor();
  return { ...base, rows: base.rows.map((row, index) => ({ ...row, entries: CATALOGUE.choice('full_view', 'thread').includes(row.permission)
    ? [named({ kind: 'group', group: crew }, index + 10), named({ kind: 'user', user: { ...BEA, address: 'acme.bea' } }, index + 100)] : [] })) };
}
beforeEach(() => { vi.clearAllMocks(); });
afterEach(async () => {
  cleanup(); useAppearance.setState(before); document.documentElement.classList.toggle('dark', darkBefore);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});

it.each(modes)('lists direct people, groups and inherited roles once with their provenance in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  const p = page();
  const view = show(<AccessRoster lines={linesOf(p, CATALOGUE, 'thread')} target={target} page={p} catalogue={CATALOGUE} changes={changes} meId={ADA.id} onTake={calls.take} onLevel={calls.level} />);
  expect(screen.getAllByText('Bea')).toHaveLength(1);
  expect(view.container).not.toHaveTextContent('acme.bea');
  expect(screen.getAllByText(i18n.t('access.roster.inheritedFrom', { place: TOPIC.name }))).not.toHaveLength(0);
  const group = screen.getByRole('button', { name: 'Crew, Acme' });
  const row = group.closest('li');
  if (row === null) throw new Error('a group has its own access row');
  expect(within(row).getByText(i18n.t('access.roster.direct'))).toBeVisible();
  expect(within(row).getByRole('button', { name: i18n.t('access.general.changeHolder', { name: 'Crew, Acme' }) })).toBeVisible();
  expect(calls.people).not.toHaveBeenCalled();
  await userEvent.click(group);
  expect(await within(row).findByText(i18n.t('access.roster.personVia', { level: i18n.t('choiceCatalog.accessLevel.view.name'), holder: 'Crew, Acme' }))).toBeVisible();
  expect(calls.people).toHaveBeenCalledTimes(1);
  expect(calls.people).toHaveBeenCalledWith(target, 'group:crew');
  expect(group).toHaveAttribute('aria-expanded', 'true');
  await userEvent.click(group);
  expect(group).toHaveAttribute('aria-expanded', 'false');
  expect(calls.take).not.toHaveBeenCalled();
});

it('keeps an uneditable inherited level read-only and offers no ineffective action', () => {
  const p = page();
  p.rows = p.rows.map((row) => ({ ...row, writable: false, can_give: false }));
  const view = show(<AccessRoster lines={linesOf(p, CATALOGUE, 'thread')} target={target} page={p} catalogue={CATALOGUE} changes={changes} meId={ADA.id} onTake={calls.take} onLevel={calls.level} />);
  expect(view.container.querySelectorAll('[aria-haspopup="menu"]')).toHaveLength(0);
  expect(calls.take).not.toHaveBeenCalled();
  expect(calls.level).not.toHaveBeenCalled();
});
