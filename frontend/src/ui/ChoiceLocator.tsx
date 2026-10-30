/** Search within one complete supplied declaration; typing never edits its committed value. */
import { Check } from 'lucide-react';
import { type KeyboardEvent, type ReactNode, useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { isComposing } from '@/lib/shortcuts';
import { choiceName, type ChoiceDetails, type ChoiceName } from '@/ui/Choice';
import { Combobox, type ComboboxOption, type ComboboxProps } from '@/ui/Combobox';
import type { FieldSlots } from '@/ui/Field';
import { useTransientLayer } from '@/ui/PromptScope';

export type ChoiceLocatorOption<Value extends string> = ChoiceName<string> & ChoiceDetails & {
  value: Value;
  fullLabel?: string;
  /** Native text when value is only an internal index (for a heterogeneous schema enum). */
  searchText?: string;
  icon?: ReactNode;
  disabled?: boolean;
};

export interface ChoiceLocatorProps<Value extends string> extends FieldSlots {
  value: Value;
  onValueChange: (value: Value) => void;
  /** This is the complete declaration/catalogue, never a resource or history page. */
  options: readonly ChoiceLocatorOption<Value>[];
  valueLabel?: string;
  placeholder?: string;
  disabled?: boolean;
  size?: 'sm' | 'md';
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  onCloseAutoFocus?: ComboboxProps<Value>['onCloseAutoFocus'];
  id?: string;
  fieldClassName?: string;
  className?: string;
}

const searchable = (text: string): string => text.normalize('NFKC').toLocaleLowerCase();

export function ChoiceLocator<Value extends string>({ value, onValueChange, options, valueLabel, placeholder,
  disabled = false, size = 'md', open, onOpenChange, onCloseAutoFocus, id, fieldClassName, className, ...field }: ChoiceLocatorProps<Value>) {
  const { t } = useTranslation();
  const [query, setQuery] = useState('');
  const selected = options.find((option) => option.value === value);
  const changedOpen = useCallback((next: boolean) => {
    if (!next) setQuery('');
    onOpenChange?.(next);
  }, [onOpenChange]);
  const [shown, changeOpen] = useTransientLayer(open, changedOpen);
  const tokens = searchable(query).split(/\s+/u).filter(Boolean);
  const matches = options.filter((option) => {
    const text = searchable([choiceName(option), option.fullLabel ?? '', typeof option.description === 'string' ? option.description : '', option.searchText ?? option.value].join(' '));
    return tokens.every((token) => text.includes(token));
  }).map((option): ComboboxOption<Value> => ({
    value: option.value, label: choiceName(option), text: option.fullLabel ?? choiceName(option),
    ...(option.fullLabel === undefined ? {} : { fullLabel: option.fullLabel }),
    ...(option.description === undefined ? {} : { description: option.description }),
    ...(option.help === undefined ? {} : { help: option.help }),
    ...(option.disabled === undefined ? {} : { disabled: option.disabled }),
    icon: <><span className="flex size-4 shrink-0 items-center justify-center text-accent-text">{option.value === value && <Check aria-hidden />}</span>{option.icon}</>,
  }));
  const searchKey = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== 'Enter' || !(event.target instanceof HTMLInputElement) || event.target.getAttribute('role') !== 'combobox') return;
    // Committing IME text is not a choice. Every other Enter belongs to this
    // locator, including a dismissed or empty list, never the enclosing form.
    if (isComposing(event.nativeEvent)) event.stopPropagation(); else event.preventDefault();
  };
  return <div className="contents" onKeyDownCapture={searchKey}>
    <Combobox {...field} query={query} onQueryChange={setQuery} open={shown} onOpenChange={changeOpen}
      onCloseAutoFocus={onCloseAutoFocus} initialActiveValue={value} displayFullValue={selected?.fullLabel} displayValue={valueLabel ?? (selected === undefined ? value : choiceName(selected))}
      placeholder={placeholder ?? t('ui.combobox.search')} empty={t('ui.combobox.empty')} disabled={disabled} size={size}
      {...(id === undefined ? {} : { id })} {...(fieldClassName === undefined ? {} : { fieldClassName })} {...(className === undefined ? {} : { className })}
      groups={[{ id: 'choices', label: '', options: matches }]} onSelect={(option) => {
        const current = options.find((candidate) => candidate.value === option.value);
        if (disabled || current === undefined || current.disabled === true) return;
        if (current.value !== value) onValueChange(current.value);
      }} />
  </div>;
}
