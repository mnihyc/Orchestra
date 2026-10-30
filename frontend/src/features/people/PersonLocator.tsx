/** One person chosen from bounded native name search; the committed identity never depends on the current hits. */
import { useQuery } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { type KeyboardEvent, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { SEARCH_MIN_CHARS, searchPeopleQuery } from '@/api/access';
import type { UserRef } from '@/generated/api-types';
import { useDebounced } from '@/lib/debounce';
import { isComposing } from '@/lib/shortcuts';
import { Avatar } from '@/ui/Avatar';
import { Combobox } from '@/ui/Combobox';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';

export function PersonLocator({ label, value, person, orgId = null, onChoose }: {
  label: string;
  value: string | null;
  person: UserRef | null;
  orgId?: string | null;
  onChoose: (person: UserRef | null) => void;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false), [typed, setTyped] = useState('');
  const text = typed.trim(), settled = useDebounced(text), waiting = text !== settled;
  // Native users/search counts Unicode characters, not UTF-16 code units, and never lists a catalogue.
  const searchable = Array.from(text).length >= SEARCH_MIN_CHARS;
  const people = useQuery({ ...searchPeopleQuery(settled, orgId), enabled: open && searchable && !waiting,
    retry: false, refetchOnWindowFocus: false, refetchOnReconnect: false });
  const failed = searchable && !waiting && people.isError;
  const hits = open && searchable && !waiting && !people.isError ? people.data ?? [] : [];
  const choose = (person: UserRef | null) => { onChoose(person); setTyped(''); };
  const selected = value === null ? t('trace.anyPerson') : person?.id === value ? person.display_name : t('trace.selectedPerson');
  const press = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Enter' || !(event.target instanceof HTMLInputElement) || event.target.getAttribute('role') !== 'combobox') return;
    // IME commits text; every other Enter stays in this locator, including an empty or dismissed list.
    if (isComposing(event.nativeEvent)) event.stopPropagation(); else event.preventDefault();
  };
  return <div className="flex min-w-0 flex-col gap-1" onKeyDownCapture={press}>
    <div className="flex min-w-0 items-end gap-1">
      <Combobox label={label} query={typed} onQueryChange={setTyped} onOpenChange={setOpen}
        displayValue={selected} placeholder={t('finder.bare.people')} fieldClassName="min-w-0 flex-1"
        loading={searchable && (waiting || people.isFetching)}
        empty={failed ? undefined : t(searchable ? 'trace.noPeople' : 'finder.namesMinimum')}
        groups={[{ id: 'people', label, options: hits.map((user) => ({ value: user.id, label: user.display_name,
          text: user.display_name, description: `@${user.username}`, icon: <Avatar text={user.avatar.text} color={user.avatar.color} size={24} /> })) }]}
        onSelect={(option) => { const found = hits.find((user) => user.id === option.value); if (found !== undefined) choose(found); }} />
      {value !== null && <IconButton size="sm" label={t('trace.clearPerson')} icon={<X aria-hidden />} onClick={() => choose(null)} />}
    </div>
    {failed && <ErrorState compact error={people.error} onRetry={() => void people.refetch({ cancelRefetch: false })} retrying={people.isFetching} />}
  </div>;
}
