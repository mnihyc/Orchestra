/** Chooses the stored part a setting field edits, separately from inherited contributions and the resolved result. */
import type { TFunction } from 'i18next';
import { isRecord } from './draftChecks';
import type { SettingsTarget } from '@/api/settings';
import { emptyOf, type Shape } from '@/features/settings/settingsModel';
import type { SettingDef, SettingPart, SettingSource, ResolvedSetting as SettingWithParts } from '@/generated/api-types';

/** A joining field is empty until this target has supplied its own part. */
export function editableValue(def: Pick<SettingDef, 'merge'> & Partial<Pick<SettingDef, 'key'>>, setting: SettingWithParts, shape: Shape): unknown {
  // This editor narrows the served list; an absent own part starts from its inherited entries.
  if (def.key === 'ask.models') return setting.value ?? [];
  if (def.merge === 'replace' || ['all', 'any', 'stricter'].includes(def.merge)) return setting.own?.value ?? setting.value ?? emptyOf(shape);
  if (setting.own !== null) return setting.own.value;
  return shape.type === 'int' ? '' : emptyOf(shape);
}

/** Whether a contribution belongs to the target, rather than a place which supplies it. */
export function ownPart(part: SettingPart, target: SettingsTarget): boolean {
  return ownSource(part.source, target);
}

export function ownSource(source: SettingSource, target: SettingsTarget): boolean {
  if (target.kind === 'system') return source.scope === 'system';
  if (target.kind === 'environment') return source.scope === 'environment_own';
  if (target.kind === 'provider') return source.scope === 'provider_own';
  return source.node?.id === target.id;
}

/** Contributions above the target stay in resolution order, even when a lock ignores one. */
export function inheritedParts(setting: SettingWithParts, target: SettingsTarget): SettingPart[] {
  return setting.parts.filter((part) => !ownPart(part, target));
}

/** Text and structured values are displayed without changing or translating the user's content. */
export function valueText(value: unknown): string {
  return typeof value === 'string' ? value : value === undefined ? '' : JSON.stringify(value, null, 2);
}

/** What the places above set for a map keyed by name, member by member as the map is joined: a later place replaces the same member of an earlier one. */
export function inheritedTargets(setting: SettingWithParts, target: SettingsTarget): Record<string, unknown> {
  const joined: Record<string, unknown> = {};
  for (const part of inheritedParts(setting, target)) {
    if (part.source.scope === 'default' || part.ignored) continue;
    if (typeof part.value === 'object' && part.value !== null && !Array.isArray(part.value)) Object.assign(joined, part.value);
  }
  return joined;
}

/** The strongest stored ancestor for this field. Later contributors win ties, as in the resolver. */
export function memberPolicy(setting: SettingWithParts, target: SettingsTarget, t: TFunction) {
  const ancestors = inheritedParts(setting, target).filter((part) => !part.ignored && !['default', 'environment'].includes(part.source.scope));
  return (path: readonly string[], order: readonly (string | boolean)[]) => {
    const [name, field] = path;
    if (name === undefined) return undefined;
    let bound: { rank: number; place: string } | undefined;
    for (const { value, source } of ancestors) {
      if (!isRecord(value)) continue;
      let rule = Object.hasOwn(value, name) ? value[name] : undefined;
      if (setting.key === 'environments.runtimes') {
        const required = Object.entries(value).some(([key, decision]) => key !== '*' && decision === 'require');
        rule ??= value['*'];
        if (required && rule !== 'require') rule = 'forbid';
        if (!required && rule === 'require') rule = 'allow';
      }
      if (field !== undefined) rule = isRecord(rule) ? rule[field] : undefined;
      const rank = order.findIndex((choice) => choice === rule);
      if (rank < 0 || (bound !== undefined && bound.rank > rank)) continue;
      const place = source.scope === 'system' ? t('settings.editor.from.system')
        : source.node?.redacted === false && source.node.name !== null ? source.node.name : t('settings.compact.hiddenSource');
      bound = { rank, place };
    }
    return bound;
  };
}
