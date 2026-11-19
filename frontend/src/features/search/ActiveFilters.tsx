/** Removable current search choices and the order control. */
import { X } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { ITEM_KINDS } from '@/api/search';
import { dayOfMoment } from '@/features/search/dates';
import type { FilterValues, PersonFilter } from '@/features/search/filterValues';
import { kindsOf, narrowing, readParams, tabOf, type SearchParams } from '@/features/search/params';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { Select } from '@/ui/Select';
import { UserChip } from '@/ui/UserChip';

export function ActiveFilters({ params, onParams, values }: { params: SearchParams; onParams: (next: SearchParams) => void; values: FilterValues }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const clear = (change: Partial<SearchParams>) => onParams(readParams({ ...params, ...change }));
  const chips: ReactNode[] = [];
  const add = (id: string, label: string, value: string | null, remove: () => void) => {
    const text = value === null ? label : t('search.filters.activeValue', { label, value });
    chips.push(<Button key={id} variant="secondary" size="sm" className="max-w-full" aria-label={t('search.filters.clearOne', { what: text })} onClick={remove} title={text}>
      <span className="min-w-0 max-w-48 truncate">{text}</span><X aria-hidden className="size-4 shrink-0" />
    </Button>);
  };
  if (params.scope !== undefined || params.document_id !== undefined || params.chat_id !== undefined) {
    add('place', t('search.filters.place'), values.place, () => clear({ scope: undefined, document_id: undefined, chat_id: undefined }));
  }
  const person = (field: PersonFilter, label: string) => {
    if (params[field] === undefined) return;
    const user = values.people[field];
    const name = values.personName(field) ?? t('search.filters.somebody');
    const text = t('search.filters.activeValue', { label, value: name });
    chips.push(<span key={field} className="inline-flex h-(--control-sm) max-w-full shrink-0 items-center gap-1 rounded-md border border-control-border bg-surface pl-2 text-sm text-text">
      <span className="shrink-0">{t('search.filters.activePrefix', { label })}</span>{user === undefined ? <span className="min-w-0 max-w-32 truncate" title={name}>{name}</span> : <UserChip user={user} />}
      <IconButton size="sm" label={t('search.filters.clearOne', { what: text })} icon={<X aria-hidden />} onClick={() => clear({ [field]: undefined })} />
    </span>);
  };
  person('owner', t('search.filters.owner'));
  person('created_by', t('search.filters.createdBy'));
  person('edited_by', t('search.filters.editedBy'));
  person('author', t('search.filters.writtenBy'));
  if (params.from !== undefined || params.to !== undefined) {
    const label = t(params.date_field === 'created' ? 'search.filters.created' : params.date_field === 'edited' ? 'search.filters.edited' : 'search.filters.written');
    const from = params.from === undefined ? '' : dayOfMoment(params.from, timeZone);
    const to = params.to === undefined ? '' : dayOfMoment(params.to, timeZone);
    const range = from !== '' && to !== '' ? `${from} – ${to}` : from !== '' ? `${t('search.filters.from')} ${from}` : `${t('search.filters.to')} ${to}`;
    add('date', label, range, () => clear({ from: undefined, to: undefined, date_field: undefined }));
  }
  if (params.file_type !== undefined) {
    const names: Record<string, 'fileImages' | 'fileVideo' | 'fileAudio' | 'filePdf' | 'fileText'> = { 'image/*': 'fileImages', 'video/*': 'fileVideo', 'audio/*': 'fileAudio', 'application/pdf': 'filePdf', 'text/*': 'fileText' };
    const key = names[params.file_type];
    add('file_type', t('search.filters.fileType'), key === undefined ? params.file_type : t(`search.filters.${key}` as const), () => clear({ file_type: undefined }));
  }
  if (params.kinds !== undefined) {
    const kinds = kindsOf(params.kinds);
    const items = kinds.filter((kind) => ITEM_KINDS.includes(kind as (typeof ITEM_KINDS)[number]));
    const customItems = items.length < ITEM_KINDS.length && (items.length > 0 || kinds.includes('chat_message') && kinds.includes('document'));
    const label = t(customItems ? 'search.filters.conversationKinds' : 'search.filters.kinds');
    const value = customItems ? t('search.filters.kindCount', { selected: formatNumber(items.length, locale), total: formatNumber(ITEM_KINDS.length, locale) })
      : kinds.length === 0 ? t('search.filters.none') : kinds.map((kind) => t(`search.kinds.${kind}` as const)).join(', ');
    add('kinds', label, value, () => clear({ kinds: undefined }));
  }
  for (const [field, key] of [
    ['title_only', 'titleOnly'], ['with_file', 'withFile'], ['mentions_me', 'mentionsMe'],
    ['shared_with_me', 'sharedWithMe'], ['participating', 'participating'], ['regex', 'regex'],
  ] as const) {
    if (params[field] === 'true') add(field, key === 'withFile' ? t('people.withFile') : t(`search.filters.${key}` as const), null, () => clear({ [field]: undefined }));
  }
  const order = [
    { value: 'newest', name: t('search.order.newest.name'), description: t('search.order.newest.description') },
    { value: 'relevance', name: t('search.order.relevance.name'), description: t('search.order.relevance.description') },
  ];
  const current = params.sort ?? 'newest';
  return <div className="flex min-w-0 items-center gap-2 py-2">
    <div aria-label={t('search.filters.active', { count: narrowing(params) })} className="flex min-w-0 flex-1 items-center gap-2 overflow-x-auto">
      {chips}
      {chips.length > 0 && <Button variant="ghost" size="sm" onClick={() => onParams({
        ...(params.q === undefined ? {} : { q: params.q }), tab: tabOf(params),
        ...(params.sort === undefined ? {} : { sort: params.sort }),
      })}>{t('search.filters.clearAll')}</Button>}
    </div>
    <Select label={t('search.order.label')} hideLabel size="sm" value={current} options={order}
      valueLabel={t('search.filters.activeValue', { label: t('search.order.label'), value: t(current === 'relevance' ? 'search.order.relevance.name' : 'search.order.newest.name') })}
      onValueChange={(value) => clear({ sort: value === 'relevance' ? 'relevance' : 'newest' })} fieldClassName="w-40 shrink-0" />
  </div>;
}
