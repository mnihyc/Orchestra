/** Choosing one place of the orgs a person belongs to, by typing part of where it lies: "Acme / Research / Web".
 *
 * A token can be kept to one place and what lies under it. The orgs, projects and topics of every root
 * org the person belongs to are listed as a path of names, narrowed as they type, and the one chosen
 * is shown with the way to take it back. Threads are left out: a token is not kept to one conversation.
 */
import { useQueries, useQuery } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { fetchTree, orgsQuery } from '@/api/nodes';
import type { AdminAccess, NodeKind, TreeNode } from '@/generated/api-types';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { Combobox, type ComboboxGroup } from '@/ui/Combobox';
import { IconButton } from '@/ui/IconButton';
import { NodeIcon } from '@/ui/NodeIcon';

/** The most places listed under what is typed: past it the list is no help in choosing, and typing more narrows it. */
const MOST_LISTED = 30;

const NO_ROWS: readonly TreeNode[] = [];

/** A place as the picker offers it. */
export interface PickedPlace {
  adminAccess?: AdminAccess;
  id: string;
  kind: NodeKind;
  /** The colour it was given, 1 to 12; `null` for the colour of its kind. */
  color: number | null;
  /** A colour of its own, where the place has one. */
  custom_color?: string | null;
  /** A project that lies in a project. */
  folder: boolean;
  /** "Acme / Research / Web". */
  path: string;
}

/** The places of one tree as paths of names, in the order of the tree. */
export function placesOfTree(rows: readonly TreeNode[], separator: string): PickedPlace[] {
  const byId = new Map(rows.map((row) => [row.id, row]));
  const pathOf = (row: TreeNode): string => {
    const names: string[] = [];
    for (let at: TreeNode | undefined = row, steps = 0; at !== undefined && steps < 64; steps += 1) {
      names.unshift(at.name);
      at = at.parent_id === null ? undefined : byId.get(at.parent_id);
    }
    return names.join(separator);
  };
  return rows
    .filter((row) => !row.deleted && row.kind !== 'thread')
    .map((row) => {
      const adminAccess = adminAccessOf(row);
      return { id: row.id, kind: row.kind, color: row.color, ...(row.custom_color === undefined ? {} : { custom_color: row.custom_color }), folder: row.kind === 'project' && (row.parent_id === null ? undefined : byId.get(row.parent_id))?.kind === 'project', path: pathOf(row), ...(adminAccess === null ? {} : { adminAccess }) };
    })
    .sort((one, other) => one.path.localeCompare(other.path));
}

export interface PlacePickerProps {
  label: string;
  hint?: string;
  value: PickedPlace | null;
  onChange: (place: PickedPlace | null) => void;
  disabled?: boolean;
  /** Restricts environment homes to their existing root org. */
  rootOrgId?: string;
  /** Offers only places of this kind: the topic a thread is made in. */
  only?: NodeKind;
}

export function PlacePicker({ label, hint, value, onChange, disabled = false, rootOrgId, only }: PlacePickerProps) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const orgs = useQuery(orgsQuery);
  const trees = useQueries({
    queries: (orgs.data ?? []).filter((org) => rootOrgId === undefined || org.id === rootOrgId).map((org) => ({
      queryKey: ['tree-places', org.id] as const,
      queryFn: ({ signal }: { signal: AbortSignal }) => fetchTree(org.id, null, signal),
    })),
    // The rows of every tree as one value, which stays the same object while nothing in it changed.
    combine: (results) => ({ trees: results.map((result) => result.data?.nodes ?? NO_ROWS), pending: results.some((result) => result.isPending) }),
  });
  const loading = orgs.isPending || trees.pending;
  const places = useMemo(() => {
    const separator = t('settings.place.separator');
    return trees.trees.flatMap((rows) => placesOfTree(rows, separator));
  }, [trees.trees, t]);
  const needle = query.trim().toLocaleLowerCase();
  const found = places
    .filter((place) => only === undefined || place.kind === only)
    .filter((place) => needle === '' || place.path.toLocaleLowerCase().includes(needle))
    .slice(0, MOST_LISTED);
  const groups: ComboboxGroup<string>[] = [
    {
      id: 'places',
      label: t('settings.place.group'),
      options: found.map((place) => ({
        value: place.id,
        label: <span className="flex min-w-0 items-center gap-2"><span>{place.path}</span><AsAdminMark of={place.adminAccess?.place} /></span>,
        text: place.path,
        icon: <NodeIcon kind={place.folder ? 'folder' : place.kind} color={place.color} own={place.custom_color} />,
        description: t(`settings.place.kinds.${place.kind}`),
      })),
    },
  ];

  if (value !== null) {
    return (
      <div className="flex flex-col gap-1">
        <span className="text-sm font-medium text-text">{label}</span>
        <div className="flex items-center gap-2 rounded-lg border border-border bg-bg-subtle px-3 py-2">
          <NodeIcon kind={value.folder ? 'folder' : value.kind} color={value.color} own={value.custom_color} className="size-4" />
          <span className="min-w-0 flex-1 text-base wrap-anywhere text-text">{value.path}</span>
          <AsAdminMark of={value.adminAccess?.place} />
          <IconButton size="sm" label={t('settings.place.clear', { path: value.path })} icon={<X aria-hidden />} disabled={disabled} onClick={() => onChange(null)} />
        </div>
      </div>
    );
  }
  return (
    <Combobox
      label={label}
      hint={hint}
      query={query}
      onQueryChange={setQuery}
      groups={groups}
      loading={loading}
      empty={t('settings.place.none')}
      placeholder={t('settings.place.placeholder')}
      disabled={disabled}
      onSelect={(option) => {
        const place = places.find((entry) => entry.id === option.value);
        if (place !== undefined) {
          onChange(place);
          setQuery('');
        }
      }}
    />
  );
}
