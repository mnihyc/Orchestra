/** A live native inventory window supplies the same searchable resource choice for pool rules. */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import type { PoolKind } from '@/generated/api-types';
import { useState } from 'react';
import { meQuery } from '@/api/auth';
import type { ResourceKind } from '@/features/resources/trail';
import { inventoryQuery } from '@/features/resources/inventoryRead';
import { useDebounced } from '@/lib/debounce';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { ResourcePickerLookup } from '@/ui/ResourcePicker';

export function PoolResourceChoice({ kind, value, onChange, label, poolKind, disabled = false }: {
  kind: ResourceKind; value: string; onChange: (id: string) => void; label: string; poolKind?: PoolKind; disabled?: boolean;
}) {
  const {t}=useTranslation();
  const reader = useQuery(meQuery);
  const [typed, setTyped] = useState('');
  const [open, setOpen] = useState(false);
  const q = useDebounced(typed.trim());
  const list = useInfiniteQuery({ ...inventoryQuery(reader.data?.id ?? '', kind, { scope: 'mine', q, sort: 'name', direction: 'asc' }, { view: 'viewable', kind: poolKind }), enabled: reader.data !== undefined && open });
  const rows = (list.data?.pages.flatMap((page) => page.items) ?? []).flatMap((row) => row.name === null ? [] : [{...row,name:row.name}]);
  const selected = rows.find((row) => row.id === value);
  return <ResourcePickerLookup label={label} pickerKey={`pool-rule-${kind}`} disabled={disabled} open={open} onOpenChange={setOpen}
    query={typed} onQueryChange={setTyped} displayValue={selected?.name ?? (value === '' ? '' : t('resourceGroups.selectionUnavailable'))} loading={list.isFetching} initialActiveValue={value}
    groups={[{ id: kind, label: '', options: rows.map((row) => ({ value: row.id, label: row.name, text: row.name,resource:{kind:row.kind,id:row.id,data:row} })) }]}
    footer={list.isError ? <ErrorState compact error={list.error} onRetry={() => void list.refetch()} /> : <Pagination hasMore={list.hasNextPage} loading={list.isFetching} onMore={() => void list.fetchNextPage()} />}
    onSelect={(option) => { onChange(option.value); setTyped(''); setOpen(false); }} />;
}
