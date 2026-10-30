/** A choice of one place or one person by typing a few letters of the name: what the filters of the search use for "where" and "by whom".
 *
 * The names come from the same request as the finder's and are shown to whoever may see them, so a place
 * the reader may not see is not offered. The control shows what is chosen, and "anything" when nothing is.
 */
import { useQuery } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { namesQuery, type NameKind } from '@/api/search';
import { describe, KIND_ICONS, wayOf } from '@/features/finder/destinations';
import { ThreadIdentity } from '@/features/people/ThreadIdentity';
import type { NameHit } from '@/generated/api-types';
import { useKnownPlaces } from '@/features/navigation/placeLabels';
import { labelOfName, labelsOfNames } from '@/features/search/labels';
import { useDebounced } from '@/lib/debounce';
import { Combobox, type ComboboxOption } from '@/ui/Combobox';
import { IconButton } from '@/ui/IconButton';
import { useErrorText } from '@/ui/errorText';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { Avatar } from '@/ui/Avatar';

/** The same pause used by the finder's name lookup. */
const PAUSE_MS = 150;

export interface NamePickerProps {
  label: string;
  kinds: readonly NameKind[];
  chosen: string | null;
  anything?: string | undefined;
  hideLabel?: boolean | undefined;
  orgId?: string | undefined;
  placeholder: string;
  nothing: string;
  onChoose: (hit: NameHit | null) => void;
  accepts?: (hit: NameHit) => boolean;
}

export function NamePicker({ label, kinds, chosen, anything, hideLabel, orgId, placeholder, nothing, onChoose, accepts }: NamePickerProps) {
  const { t } = useTranslation();
  const errorText = useErrorText();
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState('');
  const rested = useDebounced(typed.trim(), PAUSE_MS);
  const waiting = typed.trim() !== rested;
  const names = useQuery({ ...namesQuery({ q: rested, kinds, ...(orgId === undefined ? {} : { orgId }) }), enabled: open });
  const hits = open && !waiting ? (names.data ?? []).filter((hit) => accepts?.(hit) ?? true) : [];
  const known = useKnownPlaces();
  const labels = labelsOfNames(hits, known);
  const options: ComboboxOption<string>[] = typed.trim() === '' && anything !== undefined ? [{ value: 'all', label: anything, text: anything }] : [];
  for (const hit of hits) {
    const Icon = KIND_ICONS[describe(hit)];
    const name = labelOfName(hit, labels);
    const way = wayOf(hit, t, labels);
    options.push({ value: hit.id, text: name,
      label: <span className="flex min-w-0 items-center gap-2"><span className="truncate">{name}</span><AsAdminMark of={adminAccessOf(hit)?.place} /></span>,
      icon: hit.kind === 'user' && hit.avatar !== null ? <Avatar text={hit.avatar.text} color={hit.avatar.color} size={24} /> : <Icon aria-hidden />,
      description: <span className="flex flex-wrap gap-x-2"><span>{t(`finder.kinds.${hit.kind}`)}</span>{way !== '' && <span>{way}</span>}{hit.kind === 'thread' && <ThreadIdentity id={hit.id} />}</span>,
    });
  }
  const pick = (hit: NameHit | null) => { onChoose(hit); setTyped(''); };
  return <div className="flex min-w-0 items-end gap-1">
    <Combobox label={label} hideLabel={hideLabel} query={typed} onQueryChange={setTyped} onOpenChange={setOpen}
      displayValue={chosen ?? anything ?? ''} placeholder={placeholder} fieldClassName="min-w-0 flex-1"
      groups={[{ id: 'names', label: hideLabel ? label : '', options }]} loading={waiting || names.isFetching}
      error={names.isError ? errorText(names.error).message : undefined}
      empty={nothing} onSelect={(option) => pick(option.value === 'all' ? null : hits.find((hit) => hit.id === option.value) ?? null)} />
    {chosen !== null && <IconButton size="sm" label={t('search.filters.clearOne', { what: label })} icon={<X aria-hidden />} onClick={() => pick(null)} />}
  </div>;
}
