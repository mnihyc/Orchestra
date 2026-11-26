/** A child narrows the served entries by exact route, offering and placement in parent order. */
import type { SettingsTarget } from '@/api/settings';
import type { AskTarget, ResolvedSetting } from '@/generated/api-types';
import { AskModelsEditor } from './AskModelsEditor';
import { askTargetIdentity } from '@/features/ask/modelChoices';
import { objectValue, selectedBackend } from './capabilityBackendValue';
import type { ValueControlProps } from './ValueControl';

export function askModelIdentities(value: unknown): AskTarget[] {
  if (!Array.isArray(value)) return [];
  return value.flatMap((item: unknown): AskTarget[] => {
    if (objectValue(item) && objectValue(item.target) && item.target.type === 'pool' && typeof item.target.id === 'string') {
      const id = item.target.id;
      if (item.target.kind === 'model') return [{target: {type:'pool', kind:'model', id}}];
      if (item.target.kind === 'provider' && typeof item.model === 'string' && item.model.trim()) {
        return [{target:{type:'pool',kind:'provider',id},model:item.model}];
      }
    }
    if (!objectValue(item) || !objectValue(item.backend) || typeof item.backend.route !== 'string') return [];
    const backend = selectedBackend(item.backend, 'ask');
    return backend === null ? [] : [{ backend }];
  });
}
export function AskModelsSetting({ target, setting, value, label, onChange, onEdited, disabled = false }: ValueControlProps & { target: SettingsTarget; setting: ResolvedSetting }) {
  const requested = askModelIdentities(value ?? setting.value);
  const wanted = new Set(requested.map((row) => askTargetIdentity(row)));
  const rows = target.kind === 'system' ? requested : askModelIdentities(setting.value).filter((row) => wanted.has(askTargetIdentity(row)));
  return <AskModelsEditor label={label} target={target} value={rows} disabled={disabled}
    onChange={(next) => { if (!disabled) { onChange(next); onEdited?.(); } }} />;
}
