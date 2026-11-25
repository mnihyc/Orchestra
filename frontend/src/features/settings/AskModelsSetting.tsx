/** A child narrows the served entries by exact route, offering and placement in parent order. */
import type { SettingsTarget } from '@/api/settings';
import type { AskModelIdentity, ResolvedSetting } from '@/generated/api-types';
import { AskModelsEditor } from './AskModelsEditor';
import { backendIdentity, objectValue, selectedBackend } from './capabilityBackendValue';
import type { ValueControlProps } from './ValueControl';

export function askModelIdentities(value: unknown): AskModelIdentity[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item: unknown) => {
    if (!objectValue(item) || !objectValue(item.backend) || typeof item.backend.route !== 'string') return [];
    const backend = selectedBackend(item.backend, 'ask');
    return backend === null ? [] : [{ backend }];
  });
}
export function AskModelsSetting({ target, setting, value, label, onChange, onEdited, disabled = false }: ValueControlProps & { target: SettingsTarget; setting: ResolvedSetting }) {
  const requested = askModelIdentities(value ?? setting.value);
  const wanted = new Set(requested.map((row) => backendIdentity(row.backend)));
  const rows = target.kind === 'system' ? requested : askModelIdentities(setting.value).filter((row) => wanted.has(backendIdentity(row.backend)));
  return <AskModelsEditor label={label} target={target} value={rows} disabled={disabled}
    onChange={(next) => { if (!disabled) { onChange(next); onEdited?.(); } }} />;
}
