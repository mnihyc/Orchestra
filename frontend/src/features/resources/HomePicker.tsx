/** The org that would be a resource's home, chosen among the orgs a person belongs to and the orgs inside them, by where each lies.
 */
import { skipToken, useQuery } from '@tanstack/react-query';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { fetchTree, nodeQuery, orgsQuery } from '@/api/nodes';
import { departmentsOf } from '@/features/navigation/OrgSwitcher';
import { Hint } from '@/features/help/Hint';
import { choiceCopy } from '@/i18n/choices';
import { rememberedOrg, useSelectedOrg } from '@/shell/org';
import { client } from '@/api/client';
import { useTreeStore } from '@/realtime/stores/tree';
import type { Org, TreeNode } from '@/generated/api-types';
import { Combobox } from '@/ui/Combobox';
import { ErrorState } from '@/ui/ErrorState';

export interface HomeChoice {
  id: string;
  /** Where it lies: "Acme", "Acme / Research / ML". */
  label: string;
  /** Whether it is a person's own org, which has no orgs inside it and few resources. */
  personal: boolean;
  /** Whether it lies inside a root org. */
  department: boolean;
}

/** How long the tree of an org read for its departments is good: they change rarely, and a dialog is short. */
const TREE_FRESH_MS = 60_000;

/**
 * The root org in view in the left bar (or the one this device remembers for the person), when it is
 * a team's: the org the lists of resources are narrowed to for somebody who administers it.
 */
export function useViewedOrgState() {
  // The signed-in frame owns org authority; a view selector never adds a metadata read.
  const orgs = useQuery({...orgsQuery,queryFn:skipToken});
  const userId = useStore(client.session, (state) => state.userId);
  const inView = useSelectedOrg((state) => state.orgId) ?? (userId === null ? null : rememberedOrg(userId));
  const teams = orgs.data?.filter((org) => org.org_kind !== 'personal') ?? [];
  return {org:teams.find((org) => org.id === inView) ?? teams[0] ?? null,load:orgs};
}

/** The selected org is shared by resource controls; pending metadata does not revoke a saved view. */
export function useViewedOrg():Org|null {return useViewedOrgState().org;}

/** The orgs a resource could have as its home, and whether they have all been read. */
export function useHomeChoices(selected = ''): { choices: HomeChoice[]; pending: boolean; error: unknown; retry: () => void } {
  const { t } = useTranslation();
  const orgs = useQuery(orgsQuery);
  const roots: Org[] = useMemo(() => orgs.data ?? [], [orgs.data]);
  const byOrg = useTreeStore((state) => state.byOrg);
  const viewed = useSelectedOrg((state) => state.orgId);
  const cachedRoot = [...byOrg].find(([, tree]) => tree.nodes.has(selected))?.[0];
  const unknown = selected !== '' && !roots.some((org) => org.id === selected) && cachedRoot === undefined;
  const node = useQuery({ ...nodeQuery(selected), enabled: unknown, staleTime: TREE_FRESH_MS });
  const rootId = roots.find((org) => org.id === selected)?.id ?? cachedRoot ?? (unknown ? node.data?.org_id : undefined)
    ?? roots.find((org) => org.id === viewed)?.id ?? roots.find((org) => org.org_kind !== 'personal')?.id ?? roots[0]?.id ?? '';
  // Use the live tree when it is already open. Opening this picker never reads every other org.
  const tree = useQuery({
    queryKey: ['tree-places', rootId] as const,
    queryFn: ({ signal }) => fetchTree(rootId, null, signal),
    staleTime: TREE_FRESH_MS,
    enabled: selected !== '' && (!unknown || node.data !== undefined) && rootId !== '' && roots.find((org) => org.id === rootId)?.org_kind !== 'personal' && byOrg.get(rootId)?.loaded !== true,
  });
  const choices = useMemo(() => {
    const found: HomeChoice[] = [];
    roots.forEach((org) => {
      const personal = org.org_kind === 'personal';
      found.push({ id: org.id, label: personal ? t('resources.home.ownOrg', { name: org.name }) : org.name, personal, department: false });
      const rows: ReadonlyMap<string, TreeNode> = byOrg.get(org.id)?.nodes ?? new Map((org.id === rootId ? tree.data?.nodes ?? [] : []).map((row) => [row.id, row]));
      for (const { node, path } of departmentsOf(rows, org.id)) {
        found.push({ id: node.id, label: [org.name, ...path].join(t('resources.home.pathJoin')), personal: false, department: true });
      }
    });
    const current = unknown ? node.data : undefined;
    if (current?.kind === 'org' && !found.some((one) => one.id === current.id)) {
      found.push({ id: current.id, label: current.path.flatMap((part) => part.redacted || part.name === null ? [] : [part.name]).join(t('resources.home.pathJoin')), personal: false, department: true });
    }
    return found;
  }, [roots, byOrg, rootId, tree.data, unknown, node.data, t]);
  return { choices, pending: orgs.isPending, error: orgs.error ?? tree.error ?? node.error, retry: () => {
    if(orgs.isError)void orgs.refetch();if(tree.isError)void tree.refetch();if(node.isError && unknown)void node.refetch();
  } };
}

/**
 * The org to begin with: the one in view in the left bar, unless that is a person's own org, which has
 * no one in it to share with; then the first team org, and the first of all where there is none.
 */
export function defaultHome(choices: readonly HomeChoice[], inView: string | null): string {
  const viewed = choices.find((choice) => choice.id === inView);
  return (
    (viewed !== undefined && !viewed.personal ? viewed.id : undefined) ??
    choices.find((choice) => !choice.personal && !choice.department)?.id ??
    viewed?.id ??
    choices[0]?.id ??
    ''
  );
}

export interface HomePickerProps {
  value: string;
  onChange: (orgId: string) => void;
  label?: string;
  hint?: string;
  error?: string | undefined;
  disabled?: boolean;
  /** A compact creation flow needs the org names, without repeating what every home does. */
  compact?: boolean;
  /** Enrollment defaults to the caller's own home, independently of the team currently viewed. */
  preferPersonal?: boolean;
  /** An environment may take its settings from the topic or project currently being used. */
  places?: readonly { id: string; name: string }[];
}

/** A choice of the org a resource belongs to, with the one in view chosen to begin with. */
export function HomePicker({ value, onChange, label, hint, error, disabled, compact = false, preferPersonal = false, places = [] }: HomePickerProps) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const { choices, pending, error:readError, retry } = useHomeChoices(places.some((place) => place.id === value) ? '' : value);
  const userId = useStore(client.session, (state) => state.userId);
  const inView = useSelectedOrg((state) => state.orgId) ?? (userId === null ? null : rememberedOrg(userId));
  useEffect(() => {
    if (value === '' && choices.length > 0) onChange((preferPersonal ? choices.find((choice) => choice.personal)?.id : undefined) ?? defaultHome(choices, inView));
  }, [value, choices, inView, onChange, preferPersonal]);
  const options: { value: string; name: string; description?: string }[]=[...places.filter((place) => !choices.some((choice) => choice.id === place.id)).map((place) => ({ value:place.id,name:place.name })),
    ...choices.map((choice)=>({value:choice.id,...(compact?{name:choice.label}:choiceCopy(t,`resourceHome.${choice.personal?'personal':choice.department?'department':'org'}`,{name:choice.label}))}))];
  const words = query.trim().toLocaleLowerCase();
  const found = options.filter((option) => option.name.toLocaleLowerCase().includes(words)).map((option) => ({
    value: option.value, label: option.name, text: option.name,
    ...('description' in option ? { description: option.description } : {}),
  }));
  return (
    <div className="flex min-w-0 flex-col gap-2"><Combobox
      label={label ?? t('resources.home.label')}
      help={<Hint id="home" />}
      {...(hint === undefined ? {} : { hint })}
      error={error}
      query={query} onQueryChange={setQuery}
      onOpenChange={(open) => { if (!open) setQuery(''); }}
      displayValue={options.find((option) => option.value === value)?.name ?? ''}
      onSelect={(option) => { setQuery(''); onChange(option.value); }}
      disabled={disabled === true || pending}
      groups={[{ id: 'homes', label: '', options: found }]} empty={t('ui.combobox.empty')}
      placeholder={pending ? t('resources.home.loading') : t('ui.combobox.search')}
    />{readError!==null && <ErrorState compact error={readError} onRetry={retry}/>}</div>
  );
}
