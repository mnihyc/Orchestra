/** A choice of one place or one person by typing a few letters of the name: what the filters of the search use for "where" and "by whom".
 *
 * The names come from the same request as the finder's and are shown to whoever may see them, so a place
 * the reader may not see is not offered. The control shows what is chosen, and "anything" when nothing is.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { pickerNamesQuery, type NameKind } from '@/api/search';
import { describe, KIND_ICONS, wayOf } from '@/features/finder/destinations';
import { ThreadIdentity } from '@/features/people/ThreadIdentity';
import type { NameHit } from '@/generated/api-types';
import { useKnownPlaces } from '@/features/navigation/placeLabels';
import { labelOfName, labelsOfNames } from '@/features/search/labels';
import { useDebounced } from '@/lib/debounce';
import { usePickerCursorRestart } from '@/lib/pickerPages';
import { Button } from '@/ui/Button';
import { ResourcePickerLookup, type ResourceLookupOption } from '@/ui/ResourcePicker';
import { IconButton } from '@/ui/IconButton';
import { useErrorText } from '@/ui/errorText';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { Avatar } from '@/ui/Avatar';

/** The same pause used by the finder's name lookup. */
const PAUSE_MS = 150;

export interface FixedNameChoice { id: string; label: string; onChoose: () => void }

export interface NamePickerProps {
  label: string;
  pickerKey?: string;
  kinds: readonly NameKind[];
  chosen: string | null;
  anything?: string | undefined;
  hideLabel?: boolean | undefined;
  orgId?: string | undefined;
  placeholder: string;
  nothing: string;
  onChoose: (hit: NameHit | null) => void;
  accepts?: (hit: NameHit) => boolean;
  fixedChoices?: readonly FixedNameChoice[];
}

export function NamePicker({ label, pickerKey, kinds, chosen, anything, hideLabel, orgId, placeholder, nothing, onChoose, accepts, fixedChoices = [] }: NamePickerProps) {
  const { t } = useTranslation();
  const errorText = useErrorText();
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState('');
  const rested = useDebounced(typed.trim(), PAUSE_MS);
  const waiting = typed.trim() !== rested;
  const source=pickerNamesQuery({q:rested,kinds,...(orgId===undefined?{}:{orgId})});
  const names = useInfiniteQuery({...source, enabled: open });
  usePickerCursorRestart(names,source.queryKey);
  const hits = open && !waiting ? (names.data?.pages.flatMap((page)=>page.items) ?? []).filter((hit) => accepts?.(hit) ?? true) : [];
  const known = useKnownPlaces();
  const labels = labelsOfNames(hits, known);
  const options: ResourceLookupOption<string>[] = typed.trim() === '' && anything !== undefined ? [{ value: 'all', label: anything, text: anything }] : [];
  for (const choice of fixedChoices) if (typed.trim() === '' || choice.label.toLocaleLowerCase().includes(typed.trim().toLocaleLowerCase())) options.push({ value: `fixed:${choice.id}`, text: choice.label, label: choice.label });
  for (const hit of hits) {
    const Icon = KIND_ICONS[describe(hit)];
    const name = labelOfName(hit, labels);
    const way = wayOf(hit, t, labels);
    options.push({ value: hit.id, text: name,
      resource:{kind:hit.kind,id:hit.id,data:hit},
      label: <span className="flex min-w-0 items-center gap-2"><span className="truncate">{name}</span><AsAdminMark of={adminAccessOf(hit)?.place} /></span>,
      icon: hit.kind === 'user' && hit.avatar !== null ? <Avatar text={hit.avatar.text} color={hit.avatar.color} size={24} /> : <Icon aria-hidden />,
      description: <span className="flex flex-wrap gap-x-2"><span>{t(`finder.kinds.${hit.kind}`)}</span>{way !== '' && <span>{way}</span>}{hit.kind === 'thread' && <ThreadIdentity id={hit.id} />}</span>,
    });
  }
  const resources = kinds.some((kind)=>['provider','agent','environment','host','machine','template','mcp_server','mount','skill'].includes(kind));
  const pick = (hit: NameHit | null) => {setOpen(false);onChoose(hit);setTyped('');};
  return <div className="flex min-w-0 items-end gap-1">
    <ResourcePickerLookup pickerKey={pickerKey} groupHomes={resources} filterLocal={false} label={label} hideLabel={hideLabel} query={typed} onQueryChange={setTyped} onOpenChange={setOpen}
      displayValue={chosen ?? anything ?? ''} placeholder={placeholder} fieldClassName="min-w-0 flex-1"
      groups={[{ id: 'names', label: hideLabel ? label : '', options }]} loading={waiting || names.isFetching}
      error={names.isError ? errorText(names.error).message : undefined}
      footer={names.hasNextPage||names.isError?<Button size="sm" variant="ghost" pending={names.isFetching} onClick={()=>{void (names.isError&&!names.isFetchNextPageError?names.refetch():names.fetchNextPage());}}>{t(names.isError?'common.tryAgain':'ui.pagination.more')}</Button>:undefined}
      empty={nothing} onSelect={(option) => {
        const choice = fixedChoices.find((entry) => option.value === `fixed:${entry.id}`);
        if (choice !== undefined) {setOpen(false);choice.onChoose();setTyped('');}
        else pick(option.value === 'all' ? null : hits.find((hit) => hit.id === option.value) ?? null);
      }} />
    {chosen !== null && <IconButton size="sm" label={t('search.filters.clearOne', { what: label })} icon={<X aria-hidden />} onClick={() => pick(null)} />}
  </div>;
}
