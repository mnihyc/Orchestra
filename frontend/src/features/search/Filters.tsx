/** The grouped controls that narrow the search, shared by its desktop column and phone sheet. */
import { ChevronRight } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { ITEM_KINDS } from '@/api/search';
import { dayOfMoment, momentOfDay } from '@/features/search/dates';
import { NamePicker } from '@/features/search/NamePicker';
import { Hint } from '@/features/help/Hint';
import { includes, readParams, tabOf, type SearchParams, withKind } from '@/features/search/params';
import type { FilterValues, PersonFilter } from '@/features/search/filterValues';
import { useAppearance } from '@/lib/appearance';
import { useViewChoice } from '@/lib/viewChoices';
import { Checkbox } from '@/ui/Checkbox';
import { DateField } from '@/ui/DateField';
import { Select } from '@/ui/Select';
import { cx } from '@/ui/cx';

export interface FiltersProps {
  params: SearchParams;
  onParams: (next: SearchParams) => void;
  values: FilterValues;
}

const GROUPS = ['where', 'who', 'when', 'what', 'for_me', 'advanced', 'conversation_kinds'] as const;
type Group = (typeof GROUPS)[number];
const DEFAULT_GROUPS = JSON.stringify(GROUPS.filter((group) => group !== 'conversation_kinds'));

function groupsOf(value: string): Set<Group> {
  try {
    const parsed: unknown = JSON.parse(value);
    if (Array.isArray(parsed) && parsed.every((entry) => typeof entry === 'string')) {
      const available = parsed.filter((entry): entry is Group => GROUPS.includes(entry as Group));
      if (parsed.length === 0 || available.length > 0) return new Set(available);
    }
  } catch { /* An invalid old choice uses the ordinary open groups. */ }
  return new Set(GROUPS.filter((group) => group !== 'conversation_kinds'));
}

export function Filters({ params, onParams, values }: FiltersProps) {
  const { t } = useTranslation();
  const timeZone = useAppearance((state) => state.timeZone);
  const [stored, keep] = useViewChoice<string>('search', 'filter-groups', DEFAULT_GROUPS);
  const open = groupsOf(stored);
  const set = (change: Partial<SearchParams>) => onParams(readParams({ ...params, ...change }));
  const group = (id: Group, label: string, children: ReactNode) => <details key={id} open={open.has(id)}
    onToggle={(event) => {
      const next = event.currentTarget.open;
      if (next === open.has(id)) return;
      const chosen = new Set(open);
      if (next) chosen.add(id); else chosen.delete(id);
      keep(JSON.stringify(GROUPS.filter((candidate) => chosen.has(candidate))));
    }} className="min-w-0 border-b border-border pb-3">
    <summary className="focus-inset flex min-h-(--control-sm) cursor-pointer list-none items-center gap-1 rounded-md text-sm font-semibold text-text">
      <ChevronRight aria-hidden className={cx('size-4 shrink-0 text-text-muted transition-transform', open.has(id) && 'rotate-90')} />{label}
    </summary>
    <div className="flex min-w-0 flex-col gap-3 pt-2">{children}</div>
  </details>;
  const person = (field: PersonFilter, label: string) => <NamePicker label={label} kinds={['user']}
    chosen={values.personName(field)} anything={t('search.filters.authorAny')}
    placeholder={t('search.filters.authorFind')} nothing={t('search.filters.authorNothing')}
    onChoose={(hit) => { values.remember(hit); set({ [field]: hit?.id }); }} />;

  const legacyDate = params.date_field === undefined && (params.from !== undefined || params.to !== undefined);
  const date = params.date_field ?? (legacyDate ? 'written' : 'edited');
  const dateOptions = [
    { value: 'edited', name: t('search.filters.edited') },
    { value: 'created', name: t('search.filters.created') },
    ...(legacyDate ? [{ value: 'written', name: t('search.filters.written') }] : []),
  ];
  const setDay = (field: 'from' | 'to', day: string) => {
    const value = momentOfDay(day, timeZone, field === 'from' ? 'start' : 'end');
    set({ [field]: value, ...(params.date_field === undefined && !legacyDate ? { date_field: 'edited' } : {}) });
  };
  const fileOptions = [
    { value: 'any', name: t('search.filters.fileAny') },
    { value: 'image/*', name: t('search.filters.fileImages') },
    { value: 'video/*', name: t('search.filters.fileVideo') },
    { value: 'audio/*', name: t('search.filters.fileAudio') },
    { value: 'application/pdf', name: t('search.filters.filePdf') },
    { value: 'text/*', name: t('search.filters.fileText') },
  ];
  if (params.file_type !== undefined && !fileOptions.some((option) => option.value === params.file_type)) {
    fileOptions.push({ value: params.file_type, name: params.file_type });
  }

  return <div aria-label={t('search.filters.title')} className="flex min-w-0 flex-col gap-3">
    {group('where', t('search.filters.whereGroup'), <NamePicker
      label={t('search.filters.place')} kinds={['org', 'project', 'topic', 'thread', 'document', 'chat']}
      chosen={values.place} anything={t('search.filters.scopeAll')} placeholder={t('search.filters.scopeFind')}
      nothing={t('search.filters.scopeNothing')}
      onChoose={(hit) => {
        values.remember(hit);
        set({ scope: hit?.kind === 'document' || hit?.kind === 'chat' ? undefined : hit?.id,
          document_id: hit?.kind === 'document' ? hit.id : undefined,
          chat_id: hit?.kind === 'chat' ? hit.id : undefined });
      }} />)}
    {group('who', t('search.filters.whoGroup'), <>
      {person('owner', t('search.filters.owner'))}
      {person('created_by', t('search.filters.createdBy'))}
      {person('edited_by', t('search.filters.editedBy'))}
      {person('author', t('search.filters.writtenBy'))}
    </>)}
    {group('when', t('search.filters.whenGroup'), <>
      <Select label={t('search.filters.date')} value={date} options={dateOptions}
        onValueChange={(value) => set({ date_field: value === 'written' ? undefined : value as 'created' | 'edited' })} />
      <DateField label={t('search.filters.from')} value={params.from === undefined ? '' : dayOfMoment(params.from, timeZone)}
        onValueChange={(value) => setDay('from', value)} />
      <DateField label={t('search.filters.to')} value={params.to === undefined ? '' : dayOfMoment(params.to, timeZone)}
        onValueChange={(value) => setDay('to', value)} />
    </>)}
    {group('what', t('search.filters.whatGroup'), <>
      <Select label={t('search.filters.fileType')} help={<Hint id="search-file-type" popover />} value={params.file_type ?? 'any'} options={fileOptions}
        onValueChange={(value) => set({ file_type: value === 'any' ? undefined : value })} />
      <Checkbox label={t('search.filters.titleOnly')} checked={params.title_only === 'true'} onCheckedChange={(on) => set({ title_only: on ? 'true' : undefined })} />
      <Checkbox label={t('people.withFile')} checked={params.with_file === 'true'} onCheckedChange={(on) => set({ with_file: on ? 'true' : undefined })} />
      {group('conversation_kinds', t('search.filters.conversationKinds'), <fieldset className="flex flex-col gap-2">
        <legend className="sr-only">{t('search.filters.conversationKinds')}</legend>
        {ITEM_KINDS.map((kind) => <Checkbox key={kind} label={t(`search.kinds.${kind}` as const)}
          checked={includes(params, kind)} onCheckedChange={(on) => set({ tab: tabOf(params), kinds: withKind(params, kind, on) })} />)}
      </fieldset>)}
    </>)}
    {group('for_me', t('search.filters.forMeGroup'), <>
      <Checkbox label={t('search.filters.mentionsMe')} checked={params.mentions_me === 'true'} onCheckedChange={(on) => set({ mentions_me: on ? 'true' : undefined })} />
      <Checkbox label={t('search.filters.sharedWithMe')} checked={params.shared_with_me === 'true'} onCheckedChange={(on) => set({ shared_with_me: on ? 'true' : undefined })} />
      <Checkbox label={t('search.filters.participating')} checked={params.participating === 'true'} onCheckedChange={(on) => set({ participating: on ? 'true' : undefined })} />
    </>)}
    {group('advanced', t('search.filters.advancedGroup'), <Checkbox label={t('search.filters.regex')}
      checked={params.regex === 'true'} onCheckedChange={(on) => set({ regex: on ? 'true' : undefined })} />)}
  </div>;
}
