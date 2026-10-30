/** Chooses the stored part a setting field edits, separately from inherited contributions and the resolved result. */
import type { SettingsTarget } from '@/api/settings';
import { emptyOf, type Shape } from '@/features/settings/settingsModel';
import type { SettingDef, SettingPart, ResolvedSetting as SettingWithParts } from '@/generated/api-types';

/** A joining field is empty until this target has supplied its own part. */
export function editableValue(def: Pick<SettingDef, 'merge'>, setting: SettingWithParts, shape: Shape): unknown {
  if (def.merge === 'replace') return setting.own?.value ?? setting.value ?? emptyOf(shape);
  if (setting.own !== null) return setting.own.value;
  return shape.type === 'int' ? '' : emptyOf(shape);
}

/** Whether a contribution belongs to the target, rather than a place which supplies it. */
export function ownPart(part: SettingPart, target: SettingsTarget): boolean {
  if (target.kind === 'system') return part.source.scope === 'system';
  if (target.kind === 'environment') return part.source.scope === 'environment_own';
  if (target.kind === 'provider') return part.source.scope === 'provider_own';
  return part.source.node?.id === target.id;
}

/** Contributions above the target stay in resolution order, even when a lock ignores one. */
export function inheritedParts(setting: SettingWithParts, target: SettingsTarget): SettingPart[] {
  return setting.parts.filter((part) => !ownPart(part, target));
}

/** Text and structured values are displayed without changing or translating the user's content. */
export function valueText(value: unknown): string {
  return typeof value === 'string' ? value : value === undefined ? '' : JSON.stringify(value, null, 2);
}
