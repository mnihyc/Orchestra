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
import type { AskTarget } from '@/generated/api-types';
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
  const picker = (index: number, label: string, saved?: AskTarget) =>
    <AskOffering key={JSON.stringify([reader, index, saved])} saved={saved} label={label} place={place} disabled={disabled} poolName={poolName}
      onSelect={(entry) => {
        if (!system || disabled) return;
        const next = [...value];
        next[index] = entry; onChange(next); setAdding(false);
      }} />;
  const query = value.length > 10 ? search.trim().toLocaleLowerCase() : '';
  const matches = value.map((entry, index) => {
    const backend = 'backend' in entry ? entry.backend : null;
    const poolId = 'target' in entry && entry.target.type === 'pool' ? entry.target.id : null;
    const text = poolId === null ? `${backend?.offering?.offering ?? ''} ${backend?.route ?? ''}` : [poolName(poolId), 'model' in entry ? entry.model : ''].join(' ');
    return {entry, backend, poolId, index, text};
  }).filter(({text}) => text.toLocaleLowerCase().includes(query));
  return <div className="flex min-w-0 flex-col gap-2">
    {value.length > 10 && <Input hideLabel label={t('composer.model.find')} placeholder={t('composer.model.find')} value={search} onChange={(event) => setSearch(event.target.value)} />}
    <DetailPages items={matches}>{(page) => <ol aria-label={label} className="flex max-h-96 min-w-0 flex-col gap-2 overflow-y-auto">
      {page.map(({ entry, backend, poolId, index }) => {
        const name = t('modelList.entry', { number: formatNumber(index + 1, i18n.language) });
        return <li key={index} aria-label={name} className="flex min-w-0 flex-col gap-1">
          <div className="flex min-w-0 flex-wrap items-start gap-1">
            <div className="min-w-0 flex-1">{system && !disabled ? picker(index, name, entry)
              : poolId !== null ? <><ChosenPoolName id={poolId} plain fallback={t('pools.selectedModel')} />{'model' in entry && entry.model && t('startFlow.separator') + entry.model}</> : backend && <CapabilityOfferingName backend={backend} capability="ask" place={place} />}</div>
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
    {system ? <>{adding && picker(value.length, t('modelList.add'))}
      <Button size="sm" variant="ghost" disabled={disabled} onClick={() => setAdding(!adding)}>{t(adding ? 'common.cancel' : 'modelList.add')}</Button></>
      : <p className="text-xs text-text-muted">{t('ask.modelsFromAbove')}</p>}
  </div>;
}

function AskOffering({ saved, label, place, disabled, poolName, onSelect }: {
  saved: AskTarget | undefined; label: string; place: string; disabled: boolean;
  poolName: (id: string) => string; onSelect: (entry: AskTarget) => void;
}) {
  const { t } = useTranslation();
  const backend = saved && 'backend' in saved ? saved.backend : null;
  const pool = saved && 'target' in saved && saved.target.type === 'pool' ? saved.target : null;
  const [draft, setDraft] = useState<{id: string; name: string} | null>(null);
  const [model, setModel] = useState(saved && 'model' in saved ? saved.model ?? '' : backend?.offering?.offering ?? '');
  const id = draft?.id ?? pool?.id ?? null;
  const changed = model.trim() !== '' && (draft !== null || !saved || !('model' in saved) || model.trim() !== saved.model);
  const commitModel = () => {
    if (!disabled && id && changed) onSelect({target:{type:'pool',kind:'provider',id}, model:model.trim()});
  };
  return <CapabilityOfferingChoice shape={{ type:'json' }} value={backend} place={place} capability="ask" label={label} disabled={disabled}
    modelPools={{kind:'model', includeProviderPools:true, id, name: draft?.name ?? (id ? poolName(id) : undefined),
      onSelect: (id, name, kind) => {
        if (kind === 'provider') setDraft({id,name});
        else onSelect({target:{type:'pool',kind:'model',id}});
      }}}
    poolModelInput={(draft !== null || pool?.kind === 'provider') && <div className="flex items-end gap-1">
      <Input label={t('ask.model')} value={model} onChange={(event) => setModel(event.target.value)} disabled={disabled}
        onKeyDown={(event) => { if (event.key === 'Enter' && !event.nativeEvent.isComposing) {event.preventDefault(); commitModel();} }} />
      {changed && <Button size="sm" onClick={commitModel} disabled={disabled}>{t('common.save')}</Button>}
    </div>}
    onChange={(raw) => { const chosen = selectedBackend(raw, 'ask'); if (chosen) onSelect({backend:chosen}); }} />;
}
