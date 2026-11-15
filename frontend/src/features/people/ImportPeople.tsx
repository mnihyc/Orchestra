/** Source people are matched to visible accounts or kept as imported-person labels. */
import { useQuery } from '@tanstack/react-query';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { PEOPLE_SEARCH_MIN_CHARS, peopleQuery } from '@/api/chats';
import { ImportPagination } from '@/features/people/ImportPagination';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';

import type { ImportAuthors, ImportPersonMatch, ImportPlanPerson } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { useDebounced } from '@/lib/debounce';
import { formatNumber } from '@/lib/format';
import { avatarText, identityColor } from '@/lib/grapheme';
import { Combobox, type ComboboxOption } from '@/ui/Combobox';
import { useErrorText } from '@/ui/errorText';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { StateLine } from '@/ui/StateLine';
import { StatusChip } from '@/ui/StateChip';
import { Table, type Column } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';

/** A brief pause combines typing into one ordinary people search. */
const PEOPLE_SEARCH_PAUSE_MS = 150;
/** A page keeps large principal lists usable without limiting how many can be reviewed. */
const PEOPLE_PAGE = 50;
const NO_ACCOUNT = 'none';
const MATCH_COLUMN = 'match';
const MATCH_WORDS: Record<ImportPersonMatch, 'matchId' | 'matchEmail' | 'matchUsername' | 'matchChoice' | 'matchNone'> = {
  id: 'matchId', email: 'matchEmail', username: 'matchUsername', choice: 'matchChoice', none: 'matchNone',
};

function PersonMatch({ person, onChoose }: { person: ImportPlanPerson; onChoose: (user: string | null) => void }) {
  const { t } = useTranslation();
  const describe = useErrorText();
  const [typed, setTyped] = useState('');
  const [open, setOpen] = useState(false);
  const rested = useDebounced(typed.trim(), PEOPLE_SEARCH_PAUSE_MS);
  const enough = Array.from(rested).length >= PEOPLE_SEARCH_MIN_CHARS;
  const typedEnough = Array.from(typed.trim()).length >= PEOPLE_SEARCH_MIN_CHARS;
  const people = useQuery({ ...peopleQuery(null, rested), enabled: open && enough });
  const options: ComboboxOption<string>[] = [
    ...(typed.trim() === '' ? [{ value: NO_ACCOUNT, label: t('importWizard.review.importedPerson'), text: t('importWizard.review.importedPerson') }] : []),
    ...(enough && typed.trim() === rested ? people.data ?? [] : []).map((user) => ({ value: user.id, label: <UserChip user={user} card={false} />, text: `${user.display_name} @${user.username}` })),
  ];
  return <div className="flex min-w-0 flex-col gap-2">
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      {person.user !== null && <UserChip user={person.user} />}
      <StatusChip>{t(`importWizard.review.${MATCH_WORDS[person.matched]}`)}</StatusChip>
    </div>
    <Combobox size="sm" label={t('importWizard.review.choosePerson', { name: person.display_name })} hideLabel
      query={typed} onQueryChange={setTyped} onOpenChange={setOpen} placeholder={t('importWizard.review.findPerson')}
      groups={[{ id: 'people', label: t('importWizard.review.people'), options }]} loading={open && typedEnough && (typed.trim() !== rested || people.isFetching)}
      error={typedEnough && typed.trim() === rested && people.isError ? describe(people.error).message : undefined} empty={t(typed.trim() !== '' && !typedEnough ? 'importWizard.review.personSearchShort' : 'importWizard.review.noPerson')}
      onSelect={(option) => { onChoose(option.value === NO_ACCOUNT ? null : option.value); setTyped(''); }} />
  </div>;
}

export function ImportPeople({ people, authors, mayAttribute, onAuthors, onPerson }: {
  people: readonly ImportPlanPerson[];
  authors: ImportAuthors;
  mayAttribute: boolean;
  onAuthors: (authors: ImportAuthors) => void;
  onPerson: (principal: string, user: string | null) => void;
}) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const search = useCollectionSearch('import-source-people');
  const scope = search.scope + '\u0000' + search.text;
  const [page, setPage] = useState({ scope, start: 0 });
  const found = useMemo(() => {
    const words = search.text.toLocaleLowerCase().split(/\s+/u).filter(Boolean);
    return words.length === 0 ? people : people.filter((person) => {
      const text = `${person.display_name} ${person.username}`.toLocaleLowerCase();
      return words.every((word) => text.includes(word));
    });
  }, [people, search.text]);
  const start = page.scope === scope ? Math.min(page.start, Math.max(0, Math.floor((found.length - 1) / PEOPLE_PAGE) * PEOPLE_PAGE)) : 0;
  const setStart = (start: number) => setPage({ scope, start });
  const match = (person: ImportPlanPerson) => <PersonMatch person={person} onChoose={(user) => onPerson(person.principal, user)} />;
  const columns: Column<ImportPlanPerson>[] = [
    { id: 'name', header: t('importWizard.review.sourcePerson'), cell: (person, shown) => <div className="flex min-w-0 flex-col gap-1 py-1">
      <UserChip card={false} showUsername user={{ display_name: person.display_name, username: person.username, kind: 'human', avatar: { text: avatarText(person.display_name), color: identityColor(person.principal) } }} />
      <span className="text-sm text-text-muted">{t('importWizard.review.personItems', { count: person.objects, number: formatNumber(person.objects, locale) })}</span>
      {!shown.has(MATCH_COLUMN) && match(person)}
    </div> },
    { id: MATCH_COLUMN, header: t('importWizard.review.match'), from: 'md', cell: match },
  ];
  return <div className="flex min-w-0 flex-col gap-3">
    <SegmentedControl<ImportAuthors> label={t('importWizard.review.sourcePerson')} value={authors} onValueChange={onAuthors} segments={[
      { value: 'accounts', label: t('importWizard.review.byAccounts'), disabled: !mayAttribute, tooltip: !mayAttribute ? t('importWizard.review.needsAdmin') : undefined },
      { value: 'labels', label: t('importWizard.review.byLabels') },
    ]} />
    {!mayAttribute && <p className="text-sm text-text-muted">{t('importWizard.review.needsAdmin')}</p>}
    {(people.length > 10 || search.text !== '') && <CollectionSearch name={t('importWizard.review.people')} {...search} />}
    {found.length === 0 ? <StateLine compact message={t(people.length === 0 ? 'importWizard.review.noPeople' : 'ui.collection.none')} />
      : <Table label={t('importWizard.review.people')} columns={columns} rows={found.slice(start, start + PEOPLE_PAGE)} rowKey={(person) => person.principal} boundedIdentity />}
    <ImportPagination first={start > 0} more={found.length > start + PEOPLE_PAGE} onFirst={() => setStart(0)} onMore={() => setStart(start + PEOPLE_PAGE)} />
  </div>;
}
