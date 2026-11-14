/** The model's native choices use one control in the composer and in place defaults. */
import type { ReactNode } from 'react';
import { Zap } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { ContextChoice } from '@/features/composer/ContextChoice';
import { ChoiceChip, type PickRow } from '@/features/composer/PickList';
import { NONE } from '@/features/composer/SpeedAndAccess';
import type { Chooser, State } from '@/features/composer/modelState';
import { Button } from '@/ui/Button';

export function ModelParameterChoice({ state, writable, chooser = null, reason, open, onOpenChange, below, showFixed = false, onChoose }: {
  state: State; writable: boolean; chooser?: Chooser | null; reason?: string | undefined;
  open?: boolean; onOpenChange?: (open: boolean) => void; below?: ReactNode;
  /** A parameter form also shows a provider's fixed value; the composer's compact row omits a control without choices. */
  showFixed?: boolean;
  onChoose: (value: string | number | null) => void;
}) {
  const { t } = useTranslation();
  if (state.kind === 'context') return <ContextChoice state={state} writable={writable} chooser={chooser} onChoose={onChoose} />;
  const what = t(state.kind === 'speed' ? 'composer.model.speed.title' : 'composer.model.access.title');
  const none = t(state.kind === 'speed' ? 'composer.model.speed.standard' : 'composer.model.access.none');
  const choices = [...(state.allowNone === false ? [] : [{ id: null, label: none }]), ...state.open];
  if (choices.length < 2 && !showFixed) return null;
  const inForce = choices.find((choice) => choice.id === state.force) ?? choices[0];
  const value = inForce?.label ?? '';
  const enabled = writable && state.providerFixed !== true;
  const choose = (id: string | null) => { if (enabled && id !== state.force) onChoose(id); };
  if (state.kind === 'speed' && choices.length === 2) {
    const other = choices.find((choice) => choice.id !== state.force) ?? choices[1];
    return <span className="inline-flex min-w-0 max-w-full" data-composer-chip={state.kind}>
      <Button size="sm" variant="ghost" bounded blocked={!enabled} reason={reason} aria-pressed={state.force !== null} aria-label={t('composer.chip.value', { what, value })}
        icon={<Zap aria-hidden />} className="min-w-0 max-w-(--chip-width) gap-1 px-2 text-sm"
        onClick={() => { if (other !== undefined) choose(other.id); }}><span className="min-w-0 truncate">{value}</span></Button>
    </span>;
  }
  const rows: PickRow[] = choices.map((choice) => ({ id: choice.id ?? NONE, name: choice.label, checked: choice.id === state.force, disabled: !enabled,
    detail: choice.id === state.start ? t('composer.chip.startsWith') : undefined }));
  return <ChoiceChip what={what} value={value} reason={reason} open={open} onOpenChange={onOpenChange} control={state.kind} disabled={choices.length < 2}
    groups={[{ id: state.kind, label: null, rows }]} below={below}
    onChoose={(row, close) => { choose(row.id === NONE ? null : row.id); close(); }} />;
}
