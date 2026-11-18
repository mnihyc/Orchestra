/** The rows of the lists of a model and of its effort: one row for each model, in the groups its provider declared, and one for each effort of the chosen model, its default marked. */
import type { TFunction } from 'i18next';

import type { PickGroup, PickRow } from '@/features/composer/PickList';
import { formatContext } from '@/features/models/context';
import { effortDisplayName, modelDisplayName } from '@/features/models/names';
import type { ModelOption } from '@/generated/api-types';

/** A choice's wire identity is separate from its visible name. */
export const optionName = (option: Pick<ModelOption, 'display_name' | 'model_name'>): string => modelDisplayName(option.display_name);

/** The words under a model's name: where it runs and how much context it takes. */
export function modelDetail(option: ModelOption, provider: string, locale: string, t: TFunction): string {
  const window = option.parameters.context_window;
  return [provider, window === undefined || window === null ? '' : t('composer.model.window', { size: formatContext(window, locale) })].filter((part) => part !== '').join(t('startFlow.separator'));
}

/** Rows in the groups the provider declared for their models: those with none first, without a header, then each group in the order it first appears. */
export function groupedRows(items: readonly { row: PickRow; group: string | undefined }[]): PickGroup[] {
  const named = new Map<string, PickRow[]>();
  const plain: PickRow[] = [];
  for (const { row, group } of items) {
    if (group === undefined || group === '') plain.push(row);
    else named.set(group, [...(named.get(group) ?? []), row]);
  }
  return [{ id: 'plain', label: null, rows: plain }, ...[...named].map(([name, rows]) => ({ id: `group:${name}`, label: name, rows }))];
}

/** The rows of the effort list: the efforts of one model, its default marked. */
export function effortGroups(option: Pick<ModelOption, 'parameters'> | undefined, current: string | null, t: TFunction, locked = false): PickGroup[] {
  const efforts = option?.parameters.efforts ?? [];
  return [{ id: 'efforts', label: null, rows: efforts.map((value): PickRow => ({ id: value, name: effortDisplayName(value, t), keywords: value, checked: value === current, disabled: locked,
    detail: value === option?.parameters.default_effort ? t('composer.effort.default') : undefined })) }];
}

/** The effort a model is chosen with: the one in force where the new model takes it, else its own start, else the first it lists. */
export function effortAfterSwitch(current: string | null, option: Pick<ModelOption, 'parameters'>): string | null {
  const efforts = option.parameters.efforts ?? [];
  if (current !== null && efforts.includes(current)) return current;
  return option.parameters.default_effort ?? efforts[0] ?? null;
}
