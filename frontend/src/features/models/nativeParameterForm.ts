/** Model-backed declarations for a place's defaults; concrete runtime eligibility remains with the thread. */
import type { TFunction } from 'i18next';

import { contextStart, programStart, speedStart } from '@/features/composer/modelState';
import { effortDisplayName, accessProgramDisplayName } from '@/features/models/names';
import { readDescriptor } from '@/features/resources/agents/descriptor';
import { resolveSchema } from '@/features/resources/templates/schema';
import { record } from '@/features/resources/UnitCommands';
import type { ModelParameters, ParameterView, ResolvedSetting } from '@/generated/api-types';

/** Unknown runtime predicates neither invent a runtime nor hide a default meant for a later thread. */
function modelCondition(value: unknown, model: ModelParameters): boolean | undefined {
  const condition = record(value);
  for (const name of ['all', 'any'] as const) {
    if (!Array.isArray(condition[name])) continue;
    const choices = (condition[name] as unknown[]).map((part) => modelCondition(part, model));
    if (name === 'all') return choices.includes(false) ? false : choices.includes(undefined) ? undefined : true;
    return choices.includes(true) ? true : choices.includes(undefined) ? undefined : false;
  }
  if (condition.not !== undefined) { const result = modelCondition(condition.not, model); return result === undefined ? undefined : !result; }
  const lists: Record<string, readonly string[]> = {
    efforts: model.efforts ?? [], service_tiers: (model.service_tiers ?? []).map((tier) => tier.id), access_programs: model.access_programs ?? [],
    web_search: model.web_search ?? [],
  };
  if (typeof condition.model_has === 'string') {
    if (condition.model_has === 'context_max') return (model.context_max ?? 0) > 0;
    if (condition.model_has === 'input_images') return model.input_images === true;
    return lists[condition.model_has] === undefined ? undefined : lists[condition.model_has]!.length > 0;
  }
  const includes = record(condition.model_includes);
  if (typeof includes.field === 'string' && typeof includes.value === 'string') return lists[includes.field]?.includes(includes.value);
  return undefined;
}

/** Resolve only declared model lists and ranges; offered values are never choices. */
export function nativeParameterViews({ descriptor, model, native, features, scope, inherited, locale, t }: {
  descriptor: unknown; model: ModelParameters; native: unknown; features: Record<string, unknown>; scope: string | null;
  inherited: Record<string, unknown>; locale: string; t: TFunction;
}): ParameterView[] {
  const declared = record(descriptor);
  const localized = readDescriptor(descriptor, locale).parameters;
  const root = record(native);
  const fields = record(resolveSchema(root, root).properties);
  const definitions: unknown[] = Array.isArray(declared.parameters) ? declared.parameters : [];
  return definitions.flatMap((entry): ParameterView[] => {
    const def = record(entry);
    if (typeof def.key !== 'string' || def.feature !== undefined && def.feature !== null && features[String(def.feature)] !== true) return [];
    if (scope !== null && scope !== 'system' && Array.isArray(def.scopes) && !def.scopes.includes(scope)) return [];
    if (modelCondition(def.eligible_when, model) === false) return [];
    const words = localized.find((parameter) => parameter.key === def.key);
    const key = def.key;
    const rule = resolveSchema(root, fields[key]);
    const from = record(def.options).from_model;
    const choices: Record<string, { value: string; label: string }[]> = {
      efforts: (model.efforts ?? []).map((value) => ({ value, label: effortDisplayName(value, t) })),
      service_tiers: (model.service_tiers ?? []).filter((tier) => tier.id !== 'auto').map((tier) => ({ value: tier.id, label: tier.label })),
      access_programs: (model.access_programs ?? []).map((value) => ({ value, label: accessProgramDisplayName(value, t) })),
      web_search: ['disabled', ...(model.web_search ?? []).filter((mode) => mode !== 'disabled')].map((value) => ({ value, label: value })),
      live_web_search: ['disabled', ...(model.web_search ?? []).filter((mode) => mode === 'live')].map((value) => ({ value, label: value })),
    };
    const options: unknown[] | undefined = typeof from === 'string' ? choices[from] : Array.isArray(def.options) ? def.options : undefined;
    const allowed = options?.filter((option) => {
      const value = record(option).value;
      return (!Array.isArray(rule.enum) || rule.enum.includes(value)) && (!Object.hasOwn(rule, 'const') || rule.const === value);
    });
    if (typeof from === 'string' && (allowed?.length ?? 0) === 0) return [];
    const startFrom = record(def.default).from_model;
    const starts: Record<string, unknown> = {
      default_effort: model.default_effort, default_service_tier: speedStart(model), default_access_program: programStart(model), default_context: contextStart(model),
      default_web_search: model.default_web_search ?? 'disabled', default_managed_web_search: model.default_web_search ?? 'disabled',
    };
    const start = Object.hasOwn(rule, 'const') ? rule.const : Object.hasOwn(rule, 'default') ? rule.default : typeof startFrom === 'string' ? starts[startFrom] : def.default;
    const range = record(def.range);
    const largest = record(range.max).from_model === 'context_max' ? model.context_max : typeof range.max === 'number' ? range.max : undefined;
    const min = typeof range.min === 'number' ? Math.max(range.min, typeof rule.minimum === 'number' ? rule.minimum : range.min) : undefined;
    const max = typeof largest === 'number' ? Math.min(largest, typeof rule.maximum === 'number' ? rule.maximum : largest) : undefined;
    const constraint = fields[key] !== undefined && fields[key] !== false || min !== undefined ? {
      ...(root.$defs === undefined ? {} : { $defs: root.$defs }), ...rule,
      type: rule.type ?? (def.type === 'enum' ? 'string' : def.type), title: words?.title ?? key,
      ...(min === undefined ? {} : { minimum: min }), ...(max === undefined ? {} : { maximum: max }),
    } : null;
    const projection = { ...def, title: words?.title ?? def.title, description: words?.description ?? def.description,
      resolved_default: start ?? null,
      ...(allowed === undefined ? {} : { resolved_options: allowed }),
      ...(min === undefined ? {} : { resolved_range: { min, max }, provider_range: { minimum: min, ...(max === undefined ? {} : { maximum: max }) } }),
      ...(constraint === null ? {} : { provider_constraint: constraint }),
      allows_none: fields[key] === undefined || (!Array.isArray(rule.enum) || rule.enum.includes(null)) && (!Object.hasOwn(rule, 'const') || rule.const === null)
        && (rule.type === undefined || rule.type === 'null' || Array.isArray(rule.type) && rule.type.includes('null')),
    };
    return [{ key, value: inherited[key], def: projection, apply: def.apply === 'new_session' || def.apply === 'recreate_environment' ? def.apply : 'next_turn',
      source: { scope: 'default', node: null }, locked_by: null, writable: true }];
  });
}

/** A member's source follows its actual contributing part, including an inherited explicit null. */
export function nativeMemberSource(setting: ResolvedSetting, key: string): ParameterView['source'] {
  return [...setting.parts].reverse().find((part) => !part.ignored && Object.hasOwn(record(part.value), key))?.source ?? { scope: 'default', node: null };
}
