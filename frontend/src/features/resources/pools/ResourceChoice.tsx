/** A live native inventory window supplies the same searchable resource choice for pool rules. */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { meQuery } from '@/api/auth';
import type { ResourceKind } from '@/features/resources/trail';
import { inventoryQuery } from '@/features/resources/inventoryRead';
import { useDebounced } from '@/lib/debounce';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { ResourcePickerLookup } from '@/ui/ResourcePicker';

export function PoolResourceChoice({ kind, value, onChange, label, disabled = false }: {
  kind: ResourceKind; value: string; onChange: (id: string) => void; label: string; disabled?: boolean;
}) {
  const reader = useQuery(meQuery);
  const [typed, setTyped] = useState('');
  const [open, setOpen] = useState(false);
  const q = useDebounced(typed.trim());
  const list = useInfiniteQuery({ ...inventoryQuery(reader.data?.id ?? '', kind, { scope: 'all', q, sort: 'name', direction: 'asc' }, { view: 'viewable' }), enabled: reader.data !== undefined && open });
  const rows = list.data?.pages.flatMap((page) => page.items) ?? [];
  const selected = rows.find((row) => row.id === value);
  return <ResourcePickerLookup label={label} pickerKey={`pool-rule-${kind}`} disabled={disabled} open={open} onOpenChange={setOpen}
    query={typed} onQueryChange={setTyped} displayValue={selected?.name ?? value} loading={list.isFetching} initialActiveValue={value}
    groups={[{ id: kind, label: '', options: rows.map((row) => ({ value: row.id, label: row.name, text: row.name })) }]}
    footer={list.isError ? <ErrorState compact error={list.error} onRetry={() => void list.refetch()} /> : <Pagination hasMore={list.hasNextPage} loading={list.isFetching} onMore={() => void list.fetchNextPage()} />}
    onSelect={(option) => { onChange(option.value); setTyped(''); setOpen(false); }} />;
}
