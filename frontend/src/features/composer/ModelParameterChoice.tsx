/** The model's native choices use one control in the composer and in place defaults. */
import type { ReactNode } from 'react';
import { Zap } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { ContextChoice } from '@/features/composer/ContextChoice';
import { ChoiceChip, PickList, type PickRow } from '@/features/composer/PickList';
import { fromWords, NONE } from '@/features/composer/SpeedAndAccess';
import type { Chooser, State } from '@/features/composer/modelState';
import { Button } from '@/ui/Button';
import { Popover } from '@/ui/Popover';

export function ModelParameterChoice({ state, writable, chooser = null, reason, open, onOpenChange, below, showFixed = false, onChoose }: {
  state: State; writable: boolean; chooser?: Chooser | null; reason?: string | undefined;
  open?: boolean; onOpenChange?: (open: boolean) => void; below?: ReactNode;
  /** A parameter form also shows a fixed speed; the composer's compact row keeps a fixed access program readable. */
  showFixed?: boolean;
  onChoose: (value: string | number | null) => void;
}) {
  const { t } = useTranslation();
  if (state.kind === 'context') return <ContextChoice state={state} writable={writable} chooser={chooser} onChoose={onChoose} />;
  const what = t(state.kind === 'speed' ? 'composer.model.speed.title' : 'composer.model.access.title');
  const none = t(state.kind === 'speed' ? 'composer.model.speed.standard' : 'composer.model.access.none');
  const choices = [...(state.allowNone === false ? [] : [{ id: null, label: none }]), ...state.open];
  if (choices.length === 0 || state.kind === 'speed' && choices.length < 2 && state.force === null && !showFixed) return null;
  const inForce = choices.find((choice) => choice.id === state.force) ?? choices[0];
  const value = inForce?.label ?? '';
  const source = fromWords(state, chooser, t);
  const explanation = reason ?? `${value} · ${source}`;
  const enabled = writable && state.providerFixed !== true;
  const choose = (id: string | null) => { if (enabled && id !== state.force) onChoose(id); };
  const rows: PickRow[] = choices.map((choice) => ({ id: choice.id ?? NONE, name: choice.label, checked: choice.id === state.force, disabled: !enabled,
    detail: choice.id === state.force ? source : choice.id === state.start ? t('composer.chip.startsWith') : undefined }));
  const onPick = (row: PickRow) => { choose(row.id === NONE ? null : row.id); onOpenChange?.(false); };
  if (state.kind === 'speed' && choices.length === 2) {
    const other = choices.find((choice) => choice.id !== state.force) ?? choices[1];
    return <span className="inline-flex min-w-0 max-w-full" data-composer-chip={state.kind}>
      <Popover label={what} open={open} onOpenChange={onOpenChange} side="top" scroll="children" trigger={<Button size="sm" variant="ghost" bounded blocked={!enabled} reason={explanation} aria-pressed={state.force !== null} aria-label={t('composer.chip.value', { what, value })}
        icon={<Zap aria-hidden />} className="min-w-0 max-w-(--chip-width) gap-1 px-2 text-sm"
        onClick={(event) => { event.preventDefault(); if (other !== undefined) choose(other.id); onOpenChange?.(false); }}><span className="min-w-0 truncate">{value}</span></Button>}>
        <PickList label={what} groups={[{ id: state.kind, label: null, rows }]} below={below} onChoose={onPick} />
      </Popover>
    </span>;
  }
  return <ChoiceChip what={what} value={value} reason={explanation} open={open} onOpenChange={onOpenChange} control={state.kind} disabled={choices.length < 2}
    groups={[{ id: state.kind, label: null, rows }]} below={below}
    onChoose={(row, close) => { choose(row.id === NONE ? null : row.id); close(); }} />;
}
