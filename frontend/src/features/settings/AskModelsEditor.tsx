/** Ordered system model identities, or org removals, use the settings row's existing writer. */
import { ArrowDown, ArrowUp, Trash2 } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ModelEffort, type ModelEffortEntry } from '@/features/composer/ModelEffort';
import type { ModelIdentity } from '@/features/composer/modelSelection';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';

export interface AskModelsEditorProps {
  label: string;
  value: readonly ModelIdentity[];
  entries: readonly ModelEffortEntry[];
  disabled?: boolean;
  mode?: 'system' | 'narrow';
  onChange: (value: ModelIdentity[]) => void;
}

export function AskModelsEditor({ label, value, entries, disabled = false, mode = 'system', onChange }: AskModelsEditorProps) {
  const { t, i18n } = useTranslation();
  const [adding, setAdding] = useState(false);
  const rows = value.map(({ provider_id, model }) => ({ provider_id, model }));
  const move = (index: number, direction: number) => {
    const target = index + direction;
    if (disabled || mode === 'narrow' || target < 0 || target >= rows.length) return;
    const next = [...rows], entry = next[index], neighbor = next[target];
    if (entry === undefined || neighbor === undefined) return;
    next[index] = neighbor; next[target] = entry; onChange(next);
  };
  return <div className="flex min-w-0 flex-col gap-2">
    <ol aria-label={label} className="flex min-w-0 flex-col gap-2">
      {rows.map((entry, index) => {
        const name = t('modelList.entry', { number: formatNumber(index + 1, i18n.language) });
        return <li key={index} className="flex min-w-0 flex-wrap items-center gap-1">
          <span className="text-xs text-text-muted">{formatNumber(index + 1, i18n.language)}.</span>
          <ModelEffort choice={{ value: entry, entries, label: name, disabled: disabled || mode === 'narrow' || entries.length === 0, showEffort: false, onChange: (next) => {
            if (!disabled && mode === 'system' && next !== null) onChange(rows.map((held, at) => at === index ? { provider_id: next.provider_id, model: next.model } : held));
          } }} />
          <span className="ml-auto flex items-center gap-1">
            {mode === 'system' && <><IconButton size="sm" label={t('modelList.moveUp', { model: name })} icon={<ArrowUp aria-hidden />} disabled={disabled || index === 0} onClick={() => move(index, -1)} />
              <IconButton size="sm" label={t('modelList.moveDown', { model: name })} icon={<ArrowDown aria-hidden />} disabled={disabled || index === rows.length - 1} onClick={() => move(index, 1)} /></>}
            <IconButton size="sm" label={t('modelList.remove', { model: name })} icon={<Trash2 aria-hidden />} disabled={disabled} onClick={() => { if (!disabled) onChange(rows.filter((_, at) => at !== index)); }} />
          </span>
        </li>;
      })}
    </ol>
    {mode === 'narrow' ? <p className="text-xs text-text-muted">{t('ask.modelsFromAbove')}</p> : <>
      {adding && <ModelEffort open onOpenChange={(open) => { if (!open) setAdding(false); }} choice={{ value: null, entries, label: t('modelList.add'), disabled, showEffort: false,
        onChange: (next) => { if (!disabled && next !== null) { onChange([...rows, { provider_id: next.provider_id, model: next.model }]); setAdding(false); } } }} />}
      <Button size="sm" variant="ghost" disabled={disabled || adding || entries.length === 0} onClick={() => setAdding(true)}>{t('modelList.add')}</Button>
    </>}
  </div>;
}
