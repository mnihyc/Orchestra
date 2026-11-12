/** A controlled MCP choice keeps inheritance, other own entries and the native revision intact. */
import { useTranslation } from 'react-i18next';
import type { SettingWrite } from '@/api/settings';
import type { McpServer, ResolvedSetting } from '@/generated/api-types';
import { ErrorAlert } from '@/ui/ErrorState';
import { Switch } from '@/ui/Switch';

export const MCP_ENABLED = 'mcp.enabled';

function enabledMap(value: unknown): Record<string, boolean> | null {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return null;
  const entries = Object.entries(value);
  if (!entries.every((entry): entry is [string, boolean] => typeof entry[1] === 'boolean')) return null;
  return Object.fromEntries(entries);
}

/** A false entry is deliberate: removing it would allow an inherited true value through again. */
export function mcpEnableWrite(setting: ResolvedSetting, server: string, on: boolean): SettingWrite | null {
  const own = setting.own === null ? {} : enabledMap(setting.own.value);
  if (setting.key !== MCP_ENABLED || !setting.writable || enabledMap(setting.value) === null || own === null) return null;
  return { value: { ...own, [server]: on }, expectedRevision: setting.own?.revision ?? null };
}

export function McpEnabledSwitch({ server, setting, pending = false, pendingLabel, blocked, error, onApply }: {
  server: Pick<McpServer, 'id' | 'name' | 'enabled'>;
  setting: ResolvedSetting;
  pending?: boolean;
  pendingLabel?: string;
  blocked?: string;
  error?: unknown;
  onApply: (write: SettingWrite) => void;
}) {
  const { t } = useTranslation();
  const effective = enabledMap(setting.value);
  const own = setting.own === null ? {} : enabledMap(setting.own.value);
  if (setting.key !== MCP_ENABLED || effective === null || own === null) return <span role="status" className="text-sm text-text-muted">{t('mcpSwitch.unavailable')}</span>;
  const checked = effective[server.id] === true;
  const reason = !setting.writable ? t(setting.locked_by === null ? 'mcpSwitch.readOnly' : 'mcpSwitch.locked')
    : !server.enabled && !checked ? t('mcpWords.reasons.disabled') : !checked ? blocked ?? null : null;
  return <div className="flex min-w-0 flex-col gap-1">
    <Switch label={t('mcpSwitch.use', { name: server.name })} checked={checked} disabled={pending || reason !== null}
      onCheckedChange={(on) => {
        if (pending || !setting.writable || on && (!server.enabled || blocked !== undefined)) return;
        const write = mcpEnableWrite(setting, server.id, on);
        if (write !== null) onApply(write);
      }} />
    {pending && <span role="status" className="text-sm text-text-muted">{pendingLabel ?? t('mcpSwitch.saving')}</span>}
    {reason !== null && <span className="text-sm text-text-muted">{reason}</span>}
    {error !== null && error !== undefined && <ErrorAlert error={error} />}
  </div>;
}
