/** Ordered policy entries use the same paged backend selector as other model consumers. */
import { useQueryClient } from '@tanstack/react-query';
import { ArrowDown, ArrowUp, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { poolQuery } from '@/api/pools';
import type { SettingsTarget } from '@/api/settings';
import { DetailPages, ResourceDetails } from '@/features/resources/DetailSections';
import type { AskTarget, BackendRef } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { ChosenPoolName } from './ResourceChoice';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { CapabilityOfferingChoice, CapabilityOfferingName } from './CapabilityBackendChoice';
import { programBackend, selectedBackend } from './capabilityBackendValue';
import { ProgramPlacement } from './ProgramPlacement';

export interface AskModelsEditorProps {
  label: string;
  target: SettingsTarget;
  value: readonly AskTarget[];
  disabled?: boolean;
  onChange: (value: AskTarget[]) => void;
}
export function AskModelsEditor({ label, target, value, onChange, disabled = false }: AskModelsEditorProps) {
  const { t, i18n } = useTranslation();
  const [adding, setAdding] = useState(false);
  const [search, setSearch] = useState('');
  const system = target.kind === 'system', place = target.kind === 'node' ? target.id : '';
  const cache = useQueryClient();
  const reader = useStore(client.session, (state) => `${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  const poolName = (id: string) => {
    const row = cache.getQueryData(poolQuery(id, reader).queryKey);
    return row?.id === id && holds(row.access, 'pool.view') ? row.name : t('pools.selectedModel');
  };
  const move = (index: number, direction: number) => {
    if (!system || disabled) return;
    const next = [...value], other = index + direction, row = next[index], neighbor = next[other];
    if (row === undefined || neighbor === undefined) return;
    next[index] = neighbor; next[other] = row; onChange(next);
  };
  const save = (index: number, raw: unknown) => {
    const backend = selectedBackend(raw, 'ask');
    if (!system || disabled || backend === null) return;
    const next = [...value];
    next[index] = { backend }; onChange(next); setAdding(false);
  };
  const picker = (index: number, backend: BackendRef | null, name: string, poolId: string | null = null) =>
    <CapabilityOfferingChoice key={JSON.stringify([backend?.route, backend?.offering])} shape={{ type: 'json' }} value={backend} place={place} capability="ask" label={name}
      modelPools={{kind: 'model', id: poolId, name: poolId === null ? undefined : poolName(poolId), onSelect: (id) => {
        if (!system || disabled) return;
        const next = [...value];
        next[index] = {target: {type: 'pool', kind: 'model', id}};
        onChange(next); setAdding(false);
      }}} disabled={disabled} onChange={(next) => save(index, next)} />;
  const query = value.length > 10 ? search.trim().toLocaleLowerCase() : '';
  const matches = value.map((entry, index) => {
    const backend = 'backend' in entry ? entry.backend : null;
    const poolId = 'target' in entry && entry.target.type === 'pool' ? entry.target.id : null;
    const text = poolId === null ? `${backend?.offering?.offering ?? ''} ${backend?.route ?? ''}` : poolName(poolId);
    return {backend, poolId, index, text};
  }).filter(({text}) => text.toLocaleLowerCase().includes(query));
  return <div className="flex min-w-0 flex-col gap-2">
    {value.length > 10 && <Input hideLabel label={t('composer.model.find')} placeholder={t('composer.model.find')} value={search} onChange={(event) => setSearch(event.target.value)} />}
    <DetailPages items={matches}>{(page) => <ol aria-label={label} className="flex max-h-96 min-w-0 flex-col gap-2 overflow-y-auto">
      {page.map(({ backend, poolId, index }) => {
        const name = t('modelList.entry', { number: formatNumber(index + 1, i18n.language) });
        return <li key={index} aria-label={name} className="flex min-w-0 flex-col gap-1">
          <div className="flex min-w-0 flex-wrap items-start gap-1">
            <div className="min-w-0 flex-1">{system && !disabled ? picker(index, backend, name, poolId)
              : poolId !== null ? <ChosenPoolName id={poolId} plain fallback={t('pools.selectedModel')} /> : backend && <CapabilityOfferingName backend={backend} capability="ask" place={place} />}</div>
            {system && <><IconButton size="sm" label={t('modelList.moveUp', { model: name })} icon={<ArrowUp aria-hidden />} disabled={disabled || index === 0} onClick={() => move(index, -1)} />
              <IconButton size="sm" label={t('modelList.moveDown', { model: name })} icon={<ArrowDown aria-hidden />} disabled={disabled || index === value.length - 1} onClick={() => move(index, 1)} /></>}
            <IconButton size="sm" label={t('modelList.remove', { model: name })} icon={<Trash2 aria-hidden />} disabled={disabled}
              onClick={() => { if (!disabled) onChange(value.filter((_, at) => at !== index)); }} />
          </div>
          {system && backend !== null && programBackend(backend) && <ResourceDetails label={t('settingsEditors.summarizer.placement')}>
            {() => <ProgramPlacement backend={backend} target={target} disabled={disabled} onChange={(next) => save(index, next)} />}
          </ResourceDetails>}
        </li>;
      })}
    </ol>}</DetailPages>
    {system ? <>{adding && picker(value.length, null, t('modelList.add'))}
      <Button size="sm" variant="ghost" disabled={disabled} onClick={() => setAdding(!adding)}>{t(adding ? 'common.cancel' : 'modelList.add')}</Button></>
      : <p className="text-xs text-text-muted">{t('ask.modelsFromAbove')}</p>}
  </div>;
}
