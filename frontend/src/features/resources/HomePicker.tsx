/** The org that would be a resource's home, chosen among the orgs a person belongs to and the orgs inside them, by where each lies.
 */
import { skipToken, useQuery } from '@tanstack/react-query';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { nodeQuery, orgsQuery } from '@/api/nodes';
import { usePlacePages, PlacePageFoot } from '@/features/navigation/usePlacePages';
import { placesOfTree } from '@/features/settings/PlacePicker';
import { Hint } from '@/features/help/Hint';
import { choiceCopy } from '@/i18n/choices';
import { rememberedOrg, useSelectedOrg } from '@/shell/org';
import { client } from '@/api/client';
import { useTreeStore } from '@/realtime/stores/tree';
import type { NodeKind, Org } from '@/generated/api-types';
import { ResourcePickerLookup } from '@/ui/ResourcePicker';
import type { ChoiceResource } from '@/ui/DetailCard';
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
export function useHomeChoices(selected = '', search = '', open = false) {
  const { t } = useTranslation();
  const orgs = useQuery(orgsQuery);
  const roots: Org[] = useMemo(() => orgs.data ?? [], [orgs.data]);
  const byOrg = useTreeStore((state) => state.byOrg);
  const cachedRoot = [...byOrg].find(([, tree]) => tree.nodes.has(selected))?.[0];
  const unknown = orgs.data!==undefined && selected !== '' && !roots.some((org) => org.id === selected) && cachedRoot === undefined;
  const node = useQuery({ ...nodeQuery(selected), enabled: unknown });
  const viewed=useSelectedOrg((state)=>state.orgId);
  const rootId=roots.find((org)=>org.id===selected)?.id??cachedRoot??node.data?.org_id??viewed??roots.find((org)=>org.org_kind!=='personal')?.id??roots[0]?.id;
  const searched=useMemo(()=>{
    const teams=roots.filter((org)=>org.org_kind!=='personal'),needle=search.trim().toLocaleLowerCase();
    if(needle==='')return teams.filter((org)=>org.id===rootId);
    return teams;
  },[roots,search,rootId]);
  const selection=useMemo(()=>({q:search.trim(),kind:'org' as const}),[search]);
  const pages=usePlacePages(searched,selection,open);
  const choices = useMemo(() => {
    const found: HomeChoice[] = [];
    roots.forEach((org) => {
      const personal = org.org_kind === 'personal';
      found.push({ id: org.id, label: personal ? t('resources.home.ownOrg', { name: org.name }) : org.name, personal, department: false });
      const page=pages.pages.find((page)=>page.root.id===org.id);
      const cached=[...byOrg.get(org.id)?.nodes.values()??[]];
      const rows=[...new Map([...cached,...page?.rows??[]].map((row)=>[row.id,row])).values()];
      for(const place of placesOfTree(rows,t('resources.home.pathJoin')))if(place.kind==='org'&&place.id!==org.id&&(page?.matches.has(place.id)||cached.some((row)=>row.id===place.id)&&place.path.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase()))) {
        found.push({id:place.id,label:place.path,personal:false,department:true});
      }
    });
    const held=byOrg.get(cachedRoot??'');
    const selectedPlace=held===undefined?undefined:placesOfTree([...held.nodes.values()],t('resources.home.pathJoin')).find((place)=>place.id===selected);
    if(selectedPlace?.kind==='org'&&!found.some((choice)=>choice.id===selectedPlace.id))found.push({id:selectedPlace.id,label:selectedPlace.path,personal:false,department:true});
    const current = unknown ? node.data : undefined;
    if (current?.kind === 'org' && !found.some((one) => one.id === current.id)) {
      found.push({ id: current.id, label: current.path.flatMap((part) => part.redacted || part.name === null ? [] : [part.name]).join(t('resources.home.pathJoin')), personal: false, department: true });
    }
    return found;
  }, [roots, pages.pages, unknown, node.data, t,byOrg,cachedRoot,selected,search]);
  return {choices,pages:pages.pages,pending:orgs.isPending,error:orgs.error??pages.error??node.error,pageError:pages.error,frameError:orgs.error??node.error,retry:()=>{
    if(orgs.isError)void orgs.refetch();pages.retry();if(node.isError&&unknown)void node.refetch();
  }};
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
  /** The server chooses this home at the effect; choosing another entry is an explicit override. */
  defaultChoice?: string;
  /** An environment may take its settings from the topic or project currently being used. */
  places?: readonly { id: string; name: string;kind?:NodeKind }[];
}

/** A choice of the org a resource belongs to, with the one in view chosen to begin with. */
export function HomePicker({ value, onChange, label, hint, error, disabled, compact = false, preferPersonal = false, defaultChoice, places = [] }: HomePickerProps) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const [open,setOpen]=useState(false);
  const reader=useStore(client.session,(state)=>state.readerVersion);
  const [selectedLabel,setSelectedLabel]=useState<{id:string;name:string;reader:number|undefined}|null>(null);
  const { choices, pages, pending, error:readError, pageError, frameError, retry } = useHomeChoices(places.some((place) => place.id === value) ? '' : value,query,open);
  const userId = useStore(client.session, (state) => state.userId);
  const inView = useSelectedOrg((state) => state.orgId) ?? (userId === null ? null : rememberedOrg(userId));
  useEffect(() => {
    if (defaultChoice === undefined && value === '' && choices.length > 0) onChange((preferPersonal ? choices.find((choice) => choice.personal)?.id : undefined) ?? defaultHome(choices, inView));
  }, [value, choices, inView, onChange, preferPersonal, defaultChoice]);
  const options: { value: string; name: string; description?: string;resource?:ChoiceResource }[]=[...(defaultChoice===undefined?[]:[{value:'',name:defaultChoice}]),...places.filter((place) => !choices.some((choice) => choice.id === place.id)).map((place) => ({ value:place.id,name:place.name,...(place.kind===undefined?{}:{resource:{kind:place.kind,id:place.id}}) })),
    ...choices.map((choice)=>({value:choice.id,resource:{kind:'org',id:choice.id},...(compact?{name:choice.label}:choiceCopy(t,`resourceHome.${choice.personal?'personal':choice.department?'department':'org'}`,{name:choice.label}))}))];
  const words = query.trim().toLocaleLowerCase();
  const found = options.filter((option) => option.name.toLocaleLowerCase().includes(words)).map((option) => ({
    value: option.value, label: option.name, text: option.name,resource:option.resource,
    ...('description' in option ? { description: option.description } : {}),
  }));
  return (
    <div className="flex min-w-0 flex-col gap-2"><ResourcePickerLookup pickerKey="resource-home" groupHomes={false}
      label={label ?? t('resources.home.label')}
      help={<Hint id="home" />}
      {...(hint === undefined ? {} : { hint })}
      error={error}
      query={query} onQueryChange={setQuery} initialActiveValue={value}
      onOpenChange={(open) => { setOpen(open);if (!open) setQuery(''); }}
      footer={pages.some((page)=>page.back||Boolean(page.result.data?.next_cursor)||page.result.data!==undefined&&page.live!=='current')||pageError!==null?<><PlacePageFoot pages={pages}/>{pageError!==null&&<ErrorState compact error={pageError} onRetry={retry}/>}</>:undefined}
      displayValue={options.find((option) => option.value === value)?.name ?? (readError===null&&selectedLabel?.id===value&&selectedLabel.reader===reader?selectedLabel.name:'')}
      onSelect={(option) => { setSelectedLabel({id:option.value,name:option.text,reader});setOpen(false);setQuery(''); onChange(option.value); }}
      disabled={disabled === true || pending}
      groups={[{ id: 'homes', label: '', options: found }]} empty={t('ui.combobox.empty')}
      placeholder={pending ? t('resources.home.loading') : t('ui.combobox.search')}
    />{frameError!==null && <ErrorState compact error={frameError} onRetry={retry}/>}</div>
  );
}
