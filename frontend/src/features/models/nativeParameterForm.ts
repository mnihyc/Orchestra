/** Presentation of server-projected native declarations; choice admission stays with the shared native resolver. */
import type { TFunction } from 'i18next';

import { effortDisplayName, accessProgramDisplayName } from '@/features/models/names';
import { readDescriptor } from '@/features/resources/agents/descriptor';
import { record } from '@/features/resources/UnitCommands';
import type { ParameterView, ResolvedSetting } from '@/generated/api-types';

export function nativeParameterViews({ definitions, descriptor, inherited, setting, locale, t }: {
  definitions: readonly unknown[]; descriptor: unknown; inherited: Record<string, unknown>; setting: ResolvedSetting; locale: string; t: TFunction;
}): ParameterView[] {
  const localized = readDescriptor(descriptor, locale).parameters;
  return definitions.flatMap((value): ParameterView[] => {
    const def = record(value);
    if (typeof def.key !== 'string') return [];
    const key = def.key;
    const words = localized.find((parameter) => parameter.key === key);
    const from = record(def.options).from_model;
    const options = Array.isArray(def.resolved_options) ? def.resolved_options.map((option: unknown) => {
      const row = record(option);
      return typeof row.value !== 'string' ? row : { ...row, label: from === 'efforts' ? effortDisplayName(row.value, t) : from === 'access_programs' ? accessProgramDisplayName(row.value, t) : row.label };
    }) : undefined;
    return [{ key, value: inherited[key], apply: def.apply === 'new_session' || def.apply === 'recreate_environment' ? def.apply : 'next_turn',
      def: { ...def, title: words?.title ?? def.title, description: words?.description ?? def.description, ...(options === undefined ? {} : { resolved_options: options }) },
      source: [...setting.parts].reverse().find((part) => !part.ignored && Object.hasOwn(record(part.value), key))?.source ?? { scope: 'default', node: null },
      locked_by: null, writable: true }];
  });
}
