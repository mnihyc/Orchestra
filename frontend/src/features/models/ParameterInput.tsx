/** Declared native parameters share their inputs between a thread and a place's unsaved defaults. */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';

import type { ParameterView } from '@/generated/api-types';
import { choiceOption, offeredChoice } from '@/i18n/choices';
import { SchemaInput } from '@/features/resources/templates/SchemaForm';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { Input } from '@/ui/Input';
import { Switch } from '@/ui/Switch';
import type { ChoiceInheritance } from '@/ui/InheritedChoice';

export const PARAMETER_GROUPS = ['model', 'behaviour', 'safety', 'advanced', 'provider'] as const;
export interface ParameterInfo {
  title: string;
  description: string;
  group: string;
  type: string;
  options: { value: string; label: string }[] | null;
}

export function parameterInfo(parameter: ParameterView, t?: TFunction): ParameterInfo {
  const def = typeof parameter.def === 'object' && parameter.def !== null ? parameter.def as Record<string, unknown> : {};
  const choices = def.resolved_options ?? def.options;
  const options = Array.isArray(choices) ? choices.flatMap((choice: unknown) => {
    if (typeof choice !== 'object' || choice === null || !('value' in choice) || typeof choice.value !== 'string') return [];
    return [{ value: choice.value, label: 'label' in choice && typeof choice.label === 'string' ? choice.label : choice.value }];
  }) : null;
  return {
    title: parameter.key === 'web_search' && t !== undefined ? t('webSearch.title') : typeof def.title === 'string' ? def.title : parameter.key,
    description: parameter.key === 'web_search' && t !== undefined ? t('webSearch.perUseHint') : typeof def.description === 'string' ? def.description : '',
    group: PARAMETER_GROUPS.find((group) => group === def.group) ?? 'advanced',
    type: typeof def.type === 'string' ? def.type : 'string',
    options: parameter.key === 'web_search' && t !== undefined ? options?.map((option) => ({ ...option, label: offeredChoice(t, 'webSearch', option.value)?.label ?? option.label })) ?? null : options,
  };
}

export function ParameterInput({ info, parameterKey, value, constraint, providerInterface = false, disabled, open, onOpenChange, onChange, draft = false, inheritance }: {
  info: ParameterInfo; parameterKey: string; value: unknown; constraint: Record<string, unknown> | null;
  providerInterface?: boolean; disabled: boolean; open?: boolean; onOpenChange?: (open: boolean) => void;
  onChange: (value: unknown) => void; draft?: boolean;
  inheritance?: ChoiceInheritance | undefined;
}) {
  const { t } = useTranslation();
  if (constraint !== null) return <SchemaInput root={constraint} schema={{ ...constraint, title: info.title, description: info.description }}
    value={value} parts={[parameterKey]} hideHeading={providerInterface} disabled={disabled} problems={[]} driver="provider-options" onChange={onChange} />;
  if (info.type === 'bool' || info.type === 'boolean') return <Switch label={info.title} checked={value === true} disabled={disabled} onCheckedChange={onChange} />;
  const shown = typeof value === 'string' || typeof value === 'number' ? String(value) : '';
  if (info.options !== null) return <ChoiceLocator<string> inheritance={inheritance} size="sm" label={info.title} hideLabel value={typeof value === 'string' ? value : ''}
    valueLabel={info.options.some((option) => option.value === value) ? undefined : shown} open={open} onOpenChange={onOpenChange}
    onValueChange={onChange} options={info.options.map((option) => ({ value: option.value, ...choiceOption(t, 'dynamic.value', { name: option.label, context: info.title }), description: info.description }))}
    disabled={disabled} />;
  const number = info.type === 'int' || info.type === 'integer' || info.type === 'number';
  return draft ? <Input label={info.title} hideLabel type={number ? 'number' : 'text'} value={shown} disabled={disabled}
    onChange={(event) => onChange(event.target.value === '' ? undefined : number ? Number(event.target.value) : event.target.value)} />
    : <Input key={shown} label={info.title} hideLabel defaultValue={shown} readOnly={disabled}
      onBlur={(event) => { if (!disabled && event.target.value !== shown) onChange(number ? Number(event.target.value) : event.target.value); }} />;
}
