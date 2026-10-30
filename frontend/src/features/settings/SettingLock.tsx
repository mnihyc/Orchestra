/** A higher setting lock names its linked place and the actual holders of the permission which decides it there. */
import { useQuery } from '@tanstack/react-query';
import { Lock } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { pageQuery, type AccessTarget } from '@/api/access';
import { HolderView } from '@/features/access/holders';
import { holderKey, holdsOfRow } from '@/features/access/model';
import { SourceName } from '@/features/settings/SettingValue';
import type { ResolvedSetting, SettingDef } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';

function Holders({ target, permission }: { target: AccessTarget; permission: string }) {
  const { t } = useTranslation();
  const page = useQuery(pageQuery(target));
  if (page.isPending) return <p role="status" className="text-sm text-text-muted">{t('day100.settings.checkingHolders')}</p>;
  if (page.isError) return <ErrorState compact error={page.error} onRetry={() => void page.refetch()} />;
  const row = page.data.rows.find((one) => one.permission === permission);
  const holders = [...page.data.owners.map((entry) => entry.holder), ...(row === undefined ? [] : holdsOfRow(row).map((one) => one.holder))];
  return (
    <div className="flex flex-wrap items-center gap-1">
      <span className="text-sm text-text-muted">{t('day100.settings.canChange')}</span>
      {holders.length === 0 ? <span className="text-sm text-text-muted">{t('day100.settings.noHolders')}</span> : [...new Map(holders.map((holder) => [holderKey(holder), holder])).values()].map((holder) => <HolderView key={holderKey(holder)} holder={holder} target={target} />)}
    </div>
  );
}

export function SettingLock({ lock, def }: { lock: NonNullable<ResolvedSetting['locked_by']>; def: Pick<SettingDef, 'class'> }) {
  const { t } = useTranslation();
  const node = lock.node;
  const permission = def.class === 'policy' || lock.scope === 'system' ? 'settings.manage' : `${lock.scope}.manage`;
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-warning/40 bg-warning-subtle p-3">
      <p className="flex flex-wrap items-center gap-1 text-sm text-text"><Lock className="size-4" aria-hidden />{t('day100.settings.lockedAt')} <SourceName source={lock} /></p>
      <p className="text-sm text-text-muted">{t('day100.settings.required', { permission: permission === 'settings.manage' ? t('settings.permissionNames.settingsManage') : t(`permissions.${permission}`, { defaultValue: permission }) })}</p>
      {node !== null && !node.redacted && node.name !== null ? <Holders target={{ kind: node.kind, id: node.id, name: node.name }} permission={permission} /> : <p className="text-sm text-text-muted">{t('day100.settings.holdersUnavailable')}</p>}
    </div>
  );
}
