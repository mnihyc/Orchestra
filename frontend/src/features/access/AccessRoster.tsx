/** The Share list: identities, their level and its source; a group opens its actual people with the same attribution. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { ChevronDown, ChevronRight, Trash2, Users } from 'lucide-react';
import { memo, useCallback, useId, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { holderParam, peopleQuery, type AccessTarget } from '@/api/access';
import type { AccessChoice, PermissionPage } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { denial } from '@/lib/permissions';
import { UserChip } from '@/ui/UserChip';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { CompleteCollection } from '@/ui/CompleteCollection';
import { ScrollRegion } from '@/ui/ScrollRegion';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';
import { accessOption, choiceWords } from './AddHolders';
import type { Changes } from './changes';
import { gaveText, holderName, placeName, shortDate } from './holders';
import { CHOICE_ORDER, customHeld, type Catalogue, type LineHolder, type LineId, type Lines } from './model';
import { usePermissionName } from './names';

interface Props {
  lines: Lines;
  target: AccessTarget;
  page: PermissionPage;
  catalogue: Catalogue;
  changes: Changes;
  meId: string | undefined;
  onTake: (holder: LineHolder, line: LineId) => void;
  onLevel: (holder: LineHolder, choice: AccessChoice) => void;
  onOpen?: ((target: AccessTarget) => void) | undefined;
}

export function HolderPeople({ target, param, level, via }: { target: AccessTarget; param: string; level: string; via: string }) {
  const { t } = useTranslation();
  const search = useCollectionSearch('access-people:' + target.kind + ':' + target.id + ':' + param);
  const query = useInfiniteQuery(peopleQuery(target, param, search.q));
  const rows = query.data?.pages.flatMap((page) => page.items) ?? [];
  const owners = new Set(query.data?.pages.flatMap((page) => page.owners ?? []) ?? []);
  return <div className="flex min-w-0 flex-col gap-2 pl-6">
    <CollectionSearch name={via} {...search} />
    {query.isPending ? <SkeletonRows read={query} rows={2} /> : query.isError && rows.length === 0 ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />
      : <ScrollRegion kind="table" label={via} className="max-h-64 overflow-y-auto"><ul className="flex flex-col gap-2">{rows.map((user) => <li key={user.id} className="flex min-w-0 flex-col gap-1 text-sm">
        <UserChip user={user} size={24} />
        <span className="min-w-0 text-text-muted">{owners.has(user.id) ? t('access.roster.owner') : t('access.roster.personVia', { level, holder: via })}</span>
      </li>)}</ul></ScrollRegion>}
    <Pagination hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.isFetchNextPageError ? query.error : null} onMore={() => void query.fetchNextPage()} />
  </div>;
}

const RosterRow = memo(function RosterRow({ holder, line, open, onOpenPeople, ...props }: Omit<Props, 'lines'> & { holder: LineHolder; line: LineId; open:boolean; onOpenPeople:(key:string,open:boolean)=>void }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const permissionName = usePermissionName();
  const id = useId();
  const { target, page, catalogue, changes, meId, onTake, onLevel, onOpen } = props;
  const named = holder.entries.length > 0;
  const outside = holder.entries.some((entry) => entry.outside);
  const givenBy = [...new Set(holder.entries.map((entry) => gaveText(t, locale, entry)).filter((value): value is string => value !== null))];
  const mayTake = holder.holds.every((hold) => {
    const row = page.rows.find((entry) => entry.permission === hold.permission);
    return row?.writable === true || (hold.entry !== null && row?.can_give === true && meId !== undefined && hold.entry.created_by?.id === meId);
  });
  const mayLevel = holder.holds.every((hold) => page.rows.find((row) => row.permission === hold.permission)?.writable === true);
  const name = holderName(t, holder.holder, target.name);
  const count = holder.holder.kind === 'group' ? holder.holder.group.member_count : holder.holder.kind === 'role' ? holder.holder.count : null;
  const choice = line === 'view' ? 'full_view' : line === 'manage' ? 'full_manage' : 'full_interact';
  const level = line === 'custom' ? t('access.lines.custom') : accessOption(t, choice, target.kind).label;
  const param = holderParam(holder.holder);
  const entries: MenuEntry[] = mayLevel ? CHOICE_ORDER.filter((value) => catalogue.choice(value, target.kind).length > 0).map((value) => ({
    id: value, ...accessOption(t, value, target.kind),
    disabled: changes.busy ? t('common.loading') : !page.gives.choices.includes(value)
      ? denial(t, catalogue.choice(value, target.kind).find((permission) => page.rows.find((row) => row.permission === permission)?.can_give !== true) ?? 'access.manage', target.name, page.owner) : false,
    onSelect: () => onLevel(holder, value),
  })) : [];
  if (mayTake) {
    if (entries.length > 0) entries.push({ type: 'separator', id: 'remove-separator' });
    entries.push({ id: 'remove', label: t('access.line.takeOutLabel', { name }), icon: <Trash2 aria-hidden />,
    disabled: changes.busy ? t('common.loading') : false, danger: true, onSelect: () => onTake(holder, line) });
  }
  return <li className="flex min-w-0 flex-col gap-2 py-2" data-access-level={line}>
    <div className="flex min-w-0 flex-wrap items-start gap-x-3 gap-y-1">
      <div className="min-w-0 flex-1 basis-[min(100%,12rem)]">
        {holder.holder.kind === 'user' ? <UserChip user={holder.holder.user} size={24} /> : <button type="button" className="focus-inset flex min-w-0 max-w-full items-start gap-1 rounded-sm text-left text-sm font-medium text-text wrap-anywhere"
          aria-label={name} aria-expanded={param === null ? undefined : open} aria-controls={param === null ? undefined : id} disabled={param === null} onClick={() => onOpenPeople(holder.key,!open)}>
          {param !== null && <ChevronRight aria-hidden className={open ? 'mt-0.5 shrink-0 rotate-90' : 'mt-0.5 shrink-0'} />}<Users aria-hidden className="mt-0.5 shrink-0" /><span className="min-w-0">{name}</span>
          {count !== null && <span aria-hidden className="text-text-muted">{count}</span>}
        </button>}
        <div className="mt-0.5 flex flex-wrap gap-x-2 text-xs text-text-muted">
          {outside && <span>{t('access.holder.outside')}</span>}
          {named ? <span>{t('access.roster.direct')}</span> : holder.from.map((from) => {
            const words = t('access.roster.inheritedFrom', { place: placeName(t, from) });
            return from !== 'system' && !from.redacted && from.name !== null && onOpen !== undefined
              ? <button key={from.id} type="button" className="focus-inset rounded-sm text-left underline decoration-border-strong underline-offset-2 hover:text-text" onClick={() => onOpen({ kind: from.kind, id: from.id, name: from.name ?? '' })}>{words}</button>
              : <span key={from === 'system' ? from : from.id}>{words}</span>;
          })}
          {givenBy.map((words) => <span key={words}>{words}</span>)}
          {holder.until !== null && <span>{t('access.holder.until', { date: shortDate(locale, holder.until) })}</span>}
        </div>
        {line === 'custom' && <p className="mt-0.5 text-xs text-text-muted wrap-anywhere">{new Intl.ListFormat(locale, { type: 'conjunction' }).format(customHeld(holder, permissionName))}</p>}
      </div>
      {mayLevel || mayTake ? <Menu label={t('access.general.changeHolder', { name })} entries={entries} trigger={<Button size="sm" variant="ghost" disabled={changes.busy} aria-label={t('access.general.changeHolder', { name })}>{level}<ChevronDown aria-hidden /></Button>} />
        : <span className="py-1 text-sm font-medium text-text-muted">{level}</span>}
    </div>
    {open && param !== null && <div id={id}><HolderPeople target={target} param={param} level={level} via={name} /></div>}
  </li>;
});

/** Levels group the same holders as before; each visible row now says who, its level and through what. */
export function AccessRoster(props: Props) {
  const {scope}=useCollectionSearch(`access-roster:${props.target.kind}:${props.target.id}`);
  return <AccessRosterForReader key={scope} {...props}/>;
}
function AccessRosterForReader({ lines, ...props }: Props) {
  const { t } = useTranslation();
  const [expanded,setExpanded]=useState<ReadonlySet<string>>(new Set());
  const toggle=useCallback((key:string,open:boolean)=>setExpanded((before)=>{const next=new Set(before);if(open)next.add(key);else next.delete(key);return next;}),[]);
  const groups = CHOICE_LINES.filter((line) => lines[line].length > 0).map((line) => ({ line,
    label: line === 'custom' ? t('access.lines.custom') : choiceWords(t, line === 'view' ? 'full_view' : line === 'manage' ? 'full_manage' : 'full_interact', props.target.kind),
  }));
  return <section aria-label={t('access.roster.title')} data-access-roster className="flex flex-col divide-y divide-border">
    {groups.map(({ line, label }) => <div key={line} role="group" aria-label={label}><CompleteCollection label={label} items={lines[line]} pinned={lines[line].filter((holder)=>expanded.has(holder.key))}
      textOf={(row)=>`${holderName(t,row.holder,props.target.name)} ${row.holder.kind==='user'?row.holder.user.username:''} ${row.from.map((from)=>placeName(t,from)).join(' ')}`} scroll>{(rows)=><ul className="divide-y divide-border">{rows.map((holder) => <RosterRow key={holder.key} {...props} holder={holder} line={line} open={expanded.has(holder.key)} onOpenPeople={toggle}/>)}</ul>}</CompleteCollection></div>)}
  </section>;
}

const CHOICE_LINES = ['manage', 'interact', 'view', 'custom'] as const;
