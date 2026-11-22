/** Pools use the same authorized resource pager and grouping controls as their members. */
import { poolPolicyLabel } from './policyLabel';
import { Link, useNavigate } from '@tanstack/react-router';
import { Layers3 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { resourceReaders } from '@/features/resources/inventory';
import { ResourceName } from '@/features/resource-facts/ResourceFacts';
import { ResourceList, useSearchText } from '@/features/resources/ResourceList';
import type { Pool } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { PoolDialog } from './PoolDialog';

export function PoolList({ search, onSearch }: { search: { q?: string }; onSearch: (patch: { q?: string }) => void }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [creating, setCreating] = useState(false);
  const [text, setText] = useSearchText(search.q ?? '', (q) => onSearch({ q: q === '' ? undefined : q }));
  const add = <Button size="sm" onClick={() => setCreating(true)}>{t('pools.new')}</Button>;
  return <><ResourceList<Pool> kind="pools" search={{ ...search }} title={t('screens.pools.title')} icon={Layers3} lead={t('pools.listLead')}
    label={t('screens.pools.title')} readRow={resourceReaders.pools} rowKey={(row) => row.id} text={text} onText={setText}
    searchLabel={t('pools.search')} searchPlaceholder={t('pools.search')} action={add} empty={{ icon: <Layers3 aria-hidden />, title: t('pools.none'), action: add }} noMatch={t('pools.noMatch')}
    columns={[{ id: 'name', header: t('pools.name'), cell: (pool) => <ResourceName subject={{kind:'pool',pool}} asChild><Link to="/resources/pools/$id" params={{ id: pool.id }} className="font-medium hover:underline">{pool.name}</Link></ResourceName> },
      { id: 'kind', header: t('pools.kind'), cell: (pool) => t(`pools.kinds.${pool.kind}`) },
      { id: 'policy', header: t('pools.policy.label'), cell: (pool) => poolPolicyLabel(t, pool.policy) },
      { id: 'state', header: t('pools.enabled'), cell: (pool) => t(pool.enabled ? 'pools.on' : 'pools.off') }]} />
    {creating && <PoolDialog onClose={() => setCreating(false)} onCreated={(id) => void navigate({ to: '/resources/pools/$id', params: { id } })} />}</>;
}
