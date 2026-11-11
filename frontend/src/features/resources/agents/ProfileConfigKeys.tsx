/** A profile names its configuration overrides without exposing their values. */
import { useTranslation } from 'react-i18next';
import { SettingRow } from '@/ui/SettingRow';
import { ValueSummary } from '@/ui/ValueSummary';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const segment = (key: string): string => /^[A-Za-z0-9_-]+$/.test(key) ? key : JSON.stringify(key);
interface KeyPath { name: string; parent: KeyPath | null }

export function profileConfigKeys(profile: unknown): string[] {
  if (!record(profile) || !Object.hasOwn(profile, 'config') || !record(profile.config)) return [];
  const keys: string[] = [];
  const pending: { path: KeyPath; value: unknown }[] = Object.entries(profile.config).map(([key, value]) => ({ path: { name: segment(key), parent: null }, value }));
  while (pending.length > 0) {
    const next = pending.pop();
    if (next === undefined) break;
    const children = record(next.value) ? Object.entries(next.value) : [];
    if (children.length === 0) {
      const names: string[] = [];
      for (let path: KeyPath | null = next.path; path !== null; path = path.parent) names.push(path.name);
      keys.push(names.reverse().join('.'));
    } else for (const [key, value] of children) pending.push({ path: { name: segment(key), parent: next.path }, value });
  }
  return keys.sort();
}

export function ProfileConfigKeys({ profile }: { profile: unknown }) {
  const { t } = useTranslation();
  return <SettingRow name={t('agents.profile.configKeys')} readOnly value={<div data-profile-config-keys>
    <ValueSummary label={t('agents.profile.configKeys')} value={profileConfigKeys(profile)} />
  </div>} />;
}
