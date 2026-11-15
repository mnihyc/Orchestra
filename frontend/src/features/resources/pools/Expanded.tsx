/** Preview uses a sealed continuation of the common resolver and performs no selection or execution. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { poolExpandedQuery } from '@/api/pools';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';

export function PoolExpanded({ id }: { id: string }) {
  const { t } = useTranslation();
  const [cursor, setCursor] = useState<string | null>(null);
  const [text, setText] = useState('');
  const query = useQuery(poolExpandedQuery(id, cursor));
  return <div className="flex flex-col gap-3"><Input label={t('pools.searchMembers')} hideLabel value={text} onChange={(event) => setText(event.target.value)} />
    {query.isPending ? <SkeletonRows read={query} rows={3} /> : query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> :
      <ul className="max-h-[60vh] overflow-y-auto divide-y divide-border">{query.data.items.filter((row) => (row.label ?? '').toLocaleLowerCase().includes(text.toLocaleLowerCase())).map((row, index) =>
        <li key={index} className="flex flex-wrap items-baseline gap-2 py-2"><span>{row.label ?? t(row.reason === 'member_hidden' ? 'pools.hidden' : 'pools.member')}</span>
          {row.reason !== null && <span className="text-sm text-text-muted">{t(`pools.reasons.${row.reason}`, { defaultValue: t('pools.unavailable') })}</span>}</li>)}</ul>}
    <div className="flex justify-between"><Button size="sm" disabled={cursor === null || query.isFetching} onClick={() => setCursor(null)}>{t('documentFolders.beginning')}</Button>
      <Button size="sm" disabled={query.data?.next == null || query.isFetching} onClick={() => { if (query.data?.next != null) setCursor(query.data.next); }}>{t('documentFolders.next')}</Button></div>
  </div>;
}
