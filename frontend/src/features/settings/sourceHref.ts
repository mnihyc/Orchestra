/** A source-setting address uses the served identity; concealed places never supply a destination. */
import type { SettingsTarget } from '@/api/settings';
import type { SettingDef, SettingSource } from '@/generated/api-types';

export function sourceSettingHref(source: SettingSource, def: Pick<SettingDef, 'key' | 'class'>, target?: SettingsTarget, orgAddress?: string): string | null {
  const query = `setting=${encodeURIComponent(def.key)}`;
  const node = source.node;
  if (node !== null) {
    if (node.redacted || node.name === null) return null;
    switch (node.kind) {
      case 'org': return orgAddress === undefined ? null : `/org/${encodeURIComponent(orgAddress)}/settings/policies?${query}`;
      case 'project': return `/project/${node.id}/settings?section=policies&${query}`;
      case 'topic': return `/topic/${node.id}?tab=settings&section=policies&${query}`;
      case 'thread': return `/thread/${node.id}?${query}`;
    }
  }
  if (source.scope === 'system') return `/admin/settings?${query}`;
  if (source.scope === 'environment_own' && target?.kind === 'environment') return `/resources/environments/${target.id}?tab=settings&${query}`;
  if (source.scope === 'provider_own' && target?.kind === 'provider') return `/resources/providers/${target.id}?tab=settings&${query}`;
  if (source.scope === 'host_own' && target?.kind === 'host') return `/resources/hosts/${target.id}?tab=settings&${query}`;
  return null;
}
