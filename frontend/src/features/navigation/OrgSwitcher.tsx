/** Personal and team organization selection.
 *
 * The head of the left bar on a small screen, which has no rail: the root org whose tree is in view,
 * and a menu of the other orgs its reader belongs to, their own first, with what the rail has after
 * its orgs: what was shared, and the resources. And what an org shows where it has no picture, for
 * the rail and for every other place that draws one: its colour and its letters.
 */
import { Link, useNavigate } from '@tanstack/react-router';
import { Boxes, LayoutGrid, MoreHorizontal, Share2 } from 'lucide-react';
import { createContext, useContext, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { screenBuilt } from '@/shell/availability';

import type { Org, TreeNode } from '@/generated/api-types';
import { graphemes, identityColor } from '@/lib/grapheme';
import { usePlaceLabels } from '@/features/navigation/placeLabels';
import { pathTo, useOrgTree } from '@/realtime/stores/tree';
import { Avatar } from '@/ui/Avatar';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { IconButton } from '@/ui/IconButton';
import { Combobox } from '@/ui/Combobox';
import { client } from '@/api/client';
import { selectOrg } from '@/shell/org';

/**
 * The orgs inside a root org, by where each lies ("Research / ML"), in the order a person reads them:
 * what the switcher offers beside the root orgs, so that a department is one choice away.
 */
export function departmentsOf(rows: ReadonlyMap<string, TreeNode>, rootId: string): { node: TreeNode; path: string[] }[] {
  const tree = { nodes: rows, lastSeq: 0, loaded: true, error: null };
  const found: { node: TreeNode; path: string[] }[] = [];
  for (const node of rows.values()) {
    if (node.kind !== 'org' || node.parent_id === null || node.deleted || node.archived) continue;
    const above = pathTo(tree, node.id);
    if (above[0]?.id !== rootId) continue;
    found.push({ node, path: above.slice(1).map((entry) => entry.name) });
  }
  return found.sort((one, other) => one.path.join('/').localeCompare(other.path.join('/')));
}

/** The one character an org shows where it has no picture: the first of its name. */
export function orgInitial(org: Pick<Org, 'name'>): string {
  return graphemes(org.name.trim())[0]?.toLocaleUpperCase() ?? '';
}

/**
 * What each of several orgs shows where they stand side by side: the first character of its name, and
 * the next one with it where another's name begins with the same, so that "Acme" and "Apex" read "Ac"
 * and "Ap". If those still match, use the third written character beside the initial ("Bt" and
 * "Bn" for Beta and Ben). Remaining twins get a stable number; colour alone never identifies them.
 */
export function orgMarks(orgs: readonly Pick<Org, 'id' | 'name'>[]): Map<string, string> {
  const rows = orgs.map((org) => ({ id: org.id, first: orgInitial(org), letters: graphemes(org.name.trim()).filter((letter) => letter.trim() !== '') }));
  const marks = new Map(rows.map((row) => [row.id, row.first]));
  const twins = () => {
    const groups = new Map<string, typeof rows>();
    for (const row of rows) {
      const key = (marks.get(row.id) ?? '').toLocaleUpperCase();
      const group = groups.get(key);
      if (group === undefined) groups.set(key, [row]);
      else group.push(row);
    }
    return [...groups.values()].filter((group) => group.length > 1);
  };
  // Two bounded passes keep the avatar compact, including two full-width CJK graphemes.
  for (const position of [1, 2]) {
    for (const group of twins()) {
      for (const row of group) marks.set(row.id, row.first + (row.letters[position] ?? ''));
    }
  }
  for (const group of twins()) {
    // The list is ordered by activity; identifiers keep a twin's mark stable when that order changes.
    group.sort((one, other) => one.id.localeCompare(other.id));
    group.forEach((row, index) => marks.set(row.id, `${row.first}${row.letters[1] ?? ''}${index + 1}`));
  }
  return marks;
}

/** The colour of an org's mark, in the identity palette from 0: the one its icon was given, else one of its own that stays the same. */
export function orgColor(org: Pick<Org, 'id' | 'color'>): number {
  return org.color !== null && Number.isInteger(org.color) && org.color >= 1 ? org.color - 1 : identityColor(org.id);
}

/** What puts the left bar away where it is a drawer (a small screen); where it is not, it does nothing. */
export const LeaveDrawer = createContext<() => void>(() => undefined);

export interface OrgSwitcherProps {
  orgs: readonly Org[];
  /** The root org in view. */
  current: Org | null;
}

export function OrgSwitcher({ orgs, current }: OrgSwitcherProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const leaveDrawer = useContext(LeaveDrawer);
  const tree = useOrgTree(current?.id ?? null);
  const inside = useMemo(() => (current === null ? [] : departmentsOf(tree.nodes, current.id)), [tree.nodes, current]);
  // Two orgs of one name are told apart by their addresses, as on the rail.
  const placed = useMemo(() => orgs.map((org) => ({ id: org.id, name: org.name, above: [], address: org.address })), [orgs]);
  const labels = usePlaceLabels(placed, { trees: false });
  const [query, setQuery] = useState('');
  const roots = orgs.map((org) => ({
    value: org.id,
    label: org.org_kind === 'personal' ? t('navigation.ownOrg', { name: labels.get(org.id) ?? org.name }) : (labels.get(org.id) ?? org.name),
    text: org.org_kind === 'personal' ? t('navigation.ownOrg', { name: labels.get(org.id) ?? org.name }) : (labels.get(org.id) ?? org.name),
    address: org.address,
    root: org.org_id,
  }));
  const departments = inside.filter(({ node }) => !roots.some((org) => org.value === node.id)).map(({ node, path }) => ({
    value: node.id, label: path.join(t('navigation.pathJoin')), text: path.join(t('navigation.pathJoin')),
    address: node.id, root: current?.org_id ?? node.id, icon: <LayoutGrid aria-hidden />,
  }));
  const needle = query.trim().toLocaleLowerCase();
  const matches = (option: { text: string; address: string }) => `${option.text} ${option.address}`.toLocaleLowerCase().includes(needle);
  const groups = [{ id: 'orgs', label: t('navigation.orgs'), options: roots.filter(matches) },
    ...(current === null || departments.length === 0 ? [] : [{ id: 'departments', label: t('navigation.inside', { org: current.name }), options: departments.filter(matches) }])];
  const entries: MenuEntry[] = [];
  // What the rail has after the last org, where there is a rail: what was shared with the person from elsewhere, and the
  // resources. Both lead to a screen and not to another tree, so the drawer is put away: the screen that was asked for is not left under it.
  entries.push(
    ...(screenBuilt('/shared') ? [{
      id: 'shared',
      label: t('screens.shared.title'),
      icon: <Share2 aria-hidden />,
      onSelect: () => {
        void navigate({ to: '/shared' });
        leaveDrawer();
      },
    }] : []),
    {
      id: 'resources',
      label: t('screens.sections.resources'),
      icon: <Boxes aria-hidden />,
      onSelect: () => {
        void navigate({ to: '/resources/providers' });
        leaveDrawer();
      },
    },
  );
  return (
    <div className="flex h-full min-w-0 flex-1 items-center gap-1 px-2">
    {current !== null && <Link to="/org/$slug" params={{ slug: current.address }} title={t('workbench.orgActions.homeName', { name: current.name })} aria-label={t('workbench.orgActions.homeName', { name: current.name })}
      className="focus-inset flex size-(--control-sm) shrink-0 items-center justify-center rounded-md hover:bg-bg-muted"
      onClick={(event) => { if (event.button === 0 && !event.ctrlKey && !event.metaKey && !event.altKey && !event.shiftKey) { selectOrg(client.session.getState().userId, current.org_id); leaveDrawer(); } }}>
      <Avatar text={orgInitial(current)} color={orgColor(current)} kind={current.org_kind === 'personal' ? 'human' : 'org'} size={24} />
    </Link>}
    <Combobox label={t('navigation.orgs')} hideLabel fieldClassName="min-w-0 flex-1" query={query} onQueryChange={setQuery}
      displayValue={current === null ? '' : labels.get(current.id) ?? current.name} placeholder={t('navigation.chooseOrg')} groups={groups}
      onSelect={(choice) => {
        const target = [...roots, ...departments].find((option) => option.value === choice.value);
        if (target === undefined) return;
        selectOrg(client.session.getState().userId, target.root);
        setQuery(''); leaveDrawer(); void navigate({ to: '/org/$slug', params: { slug: target.address } });
      }} />
    <Menu label={t('navigation.otherPlaces')} entries={entries}
      trigger={<IconButton size="sm" label={t('navigation.otherPlaces')} icon={<MoreHorizontal aria-hidden />} />} />
    </div>
  );
}
