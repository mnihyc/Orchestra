/** One place chosen with the shared keyboard tree, followed by the server's current audience preview. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { namesQuery } from '@/api/search';
import type { NameHit } from '@/generated/api-types';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';
import { Tree } from '@/ui/Tree';
import { PortFault } from './Fault';

export function PlaceChoice({ chosen, onChoose }: { chosen: NameHit | null; onChoose: (hit: NameHit | null) => void }) {
  const { t } = useTranslation();
  const [typed, setTyped] = useState('');
  const names = useQuery(namesQuery({ kinds: ['org', 'project', 'topic'], q: typed }));
  return <div className="flex flex-col gap-2">
    <Input label={t('ports.searchPlace')} value={typed} onChange={(event) => setTyped(event.target.value)} />
    {names.isPending ? <SkeletonRows rows={2} /> : names.isError ? <PortFault error={names.error} /> : names.data.length === 0 ? <EmptyState compact title={t('ports.noPlaces')} /> :
      <Tree label={t('ports.choosePlace')} rows={names.data.map((item) => ({ id: item.id, depth: 0, parentId: null, expandable: false, expanded: false, item, tick: chosen?.id === item.id }))} currentId={chosen?.id} onExpandedChange={() => undefined} onTick={(row, on) => onChoose(on ? row.item : null)} nameOf={(item) => item.name} renderRow={(row) => <span className="truncate text-sm">{row.item.name}</span>} />}
  </div>;
}
