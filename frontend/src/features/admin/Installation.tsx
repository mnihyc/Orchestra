/** Release lookups and verified database backup records, with operator commands kept as copyable text. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { checkRelease, installationRecordsQuery, systemSettingsQuery } from '@/api/admin';
import { BackupRestoreList } from '@/features/admin/BackupRestoreList';
import { Facts, CommandBox } from '@/features/resources/Facts';
import { SettingsEditor } from '@/features/settings/Editor';
import type { DatabaseBackupRecord, InstallationRecords, ReleaseCheck } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { formatExact, formatSpan, useNow } from '@/lib/time';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';

/** These are the existing updater's operator command and its paired manifest suffix. */
const BACKUP_COMMAND = './update.sh backup';
const OPERATIONAL_SETTINGS = ['release.check_days', 'backup.warn_days'];
const RELEASE_CONFIGURATION = 'config/orch.toml → [release] url';
const RELEASE_ENVIRONMENT = 'ORCH_RELEASE_URL';

function ReleaseFacts({ release }: { release: ReleaseCheck | null }) {
  const { t } = useTranslation();
  if (release === null) return <p className="text-sm text-text-muted">{t('installationRecords.notChecked')}</p>;
  return <div className="flex flex-col gap-3">
    {release.error !== null && <InlineAlert tone="warning" title={t('installationRecords.checkFailed')}>{release.error}</InlineAlert>}
    {release.newer === true && release.offered_version !== null ? <p className="font-medium text-text">{t('installationRecords.newer', { version: release.offered_version })}</p> : release.newer === false ? <p className="text-text">{t('installationRecords.current')}</p> : <p className="text-sm text-text-muted">{t('installationRecords.noOffer')}</p>}
    <Facts rows={[
      { name: release.offered_version === null ? t('installationRecords.attempted') : t('installationRecords.checked'), value: <RelativeTime at={release.checked_at} /> },
      { name: t('installationRecords.installed'), value: release.installed_version },
      ...(release.offered_version === null ? [] : [{ name: t('componentUpdates.latestRelease'), value: release.offered_version }]),
      { name: t('installationRecords.source'), value: release.source === '' ? t('installationRecords.noSource') : <code className="text-sm wrap-anywhere">{release.source}</code> },
    ]} />
    {release.error !== null && release.offered_version !== null && <p className="text-sm text-text-muted">{t('installationRecords.previousOffer')}</p>}
    {release.notes_url && <a href={release.notes_url} target="_blank" rel="noreferrer" className="text-accent hover:underline">{t('componentUpdates.releaseNotes')}</a>}
    {release.command !== null && <CommandBox command={release.command} label={t('installationRecords.copyUpdate')} />}
    <p className="text-sm text-text-muted">{t('componentUpdates.serverCommand')}</p>
  </div>;
}

function BackupFacts({ backup }: { backup: DatabaseBackupRecord }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const now = useNow();
  const context = { locale, timeZone };
  return <>
    <Facts rows={[
      { name: t('installationRecords.completed'), value: <time dateTime={backup.completed_at}>{formatExact(backup.completed_at, context)}</time> },
      { name: t('installationRecords.age'), value: formatSpan(now - Date.parse(backup.completed_at), context) },
      { name: t('installationRecords.databaseAt'), value: <time dateTime={backup.database_at}>{formatExact(backup.database_at, context)}</time> },
      { name: t('installationRecords.dump'), value: <code className="text-sm wrap-anywhere">{backup.dump_name}</code> },
      { name: t('installationRecords.size'), value: formatBytes(backup.bytes, locale) },
      { name: t('installationRecords.coverage'), value: backup.database_only ? t('installationRecords.databaseOnly') : t('installationRecords.unknownCoverage') },
      { name: t('installationRecords.releaseSchema'), value: `${backup.release} · ${backup.schema}` },
      { name: t('installationRecords.cause'), value: t(`installationRecords.causes.${backup.cause}`) },
    ]} />
    {backup.last_failure !== null && <InlineAlert tone="warning" title={t('installationRecords.laterFailure')}><p><RelativeTime at={backup.last_failure.at} /></p><p>{backup.last_failure.reason}</p></InlineAlert>}
  </>;
}

function RestoreList({ backup }: { backup: DatabaseBackupRecord | null }) {
  return <BackupRestoreList expected={backup} />;
}

function RecordSections({ records, refreshing }: { records: InstallationRecords; refreshing: boolean }) {
  const { t } = useTranslation();
  const [dialog, setDialog] = useState<'settings' | 'configuration' | 'restore' | null>(null);
  const settings = useQuery(systemSettingsQuery);
  const days = settings.data?.find((entry) => entry.key === 'release.check_days')?.value;
  const warningDays = settings.data?.find((entry) => entry.key === 'backup.warn_days')?.value;
  const check = useMutation({ mutationFn: checkRelease });
  return <>
    {refreshing && <InlineAlert tone="info">{t('installationRecords.checkingRecords')}</InlineAlert>}
    <Section id="admin-release-check" title={t('installationRecords.releaseTitle')} lead={t('installationRecords.releaseAbout')} action={<Button pending={check.isPending} onClick={() => check.mutate()}>{t(records.release?.error === null || records.release === null ? 'installationRecords.check' : 'installationRecords.retryCheck')}</Button>}>
      <ReleaseFacts release={records.release} />
      {check.error !== null && <ErrorAlert error={check.error} />}
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2 text-sm text-text-muted">
        <span>{settings.isError || typeof days !== 'number' ? t('installationRecords.frequencyUnknown') : days === 0 ? t('installationRecords.off') : t('installationRecords.every', { count: days })}</span>
        <span>{t('componentUpdates.hourly')}</span>
        <Button size="sm" variant="ghost" onClick={() => setDialog('settings')}>{t('installationRecords.checkSettings')}</Button>
        <Button size="sm" variant="ghost" onClick={() => setDialog('configuration')}>{t('installationRecords.configure')}</Button>
      </div>
      {settings.isError && <ErrorState compact error={settings.error} onRetry={() => void settings.refetch()} retrying={settings.isRefetching} />}
    </Section>
    <Section id="admin-database-backup" title={t('installationRecords.backupTitle')}>
      {records.backup === null ? <p className="text-text">{t('installationRecords.noBackup')}</p> : <BackupFacts backup={records.backup} />}
      {records.backup_warning && <InlineAlert tone="warning">{t(records.backup === null ? 'installationRecords.missingBackupWarning' : 'installationRecords.backupWarning')}</InlineAlert>}
      {records.backup_error !== null && <InlineAlert tone="warning" title={t('installationRecords.recordError')}>{records.backup_error}</InlineAlert>}
      <p className="text-sm text-text-muted">{t('installationRecords.noBackupAct')}</p>
      <CommandBox command={BACKUP_COMMAND} label={t('installationRecords.copyBackup')} />
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        <Button size="sm" variant="ghost" onClick={() => setDialog('restore')}>{t('installationRecords.restoreList')}</Button>
        <span className="text-sm text-text-muted">{settings.isError || typeof warningDays !== 'number' ? t('installationRecords.warningUnknown') : t('installationRecords.warnAfter', { count: warningDays })}</span>
      </div>
    </Section>
    {dialog !== null && <Dialog open onOpenChange={(open) => { if (!open) setDialog(null); }} size="lg" title={t(dialog === 'settings' ? 'installationRecords.checkSettings' : dialog === 'configuration' ? 'installationRecords.configure' : 'installationRecords.restoreList')}>
      {dialog === 'settings' ? <SettingsEditor target={{ kind: 'system' }} scope="system" only={OPERATIONAL_SETTINGS} place={{ name: t('admin.nav.groups.system'), owner: null, access: null, kind: null }} /> : dialog === 'restore' ? <RestoreList backup={records.backup} /> : <div className="flex flex-col gap-3 text-sm text-text">
        <p>{t('installationRecords.configureAbout')}</p><code>{RELEASE_CONFIGURATION}</code><code>{RELEASE_ENVIRONMENT}</code><p>{t('installationRecords.configureRestart')}</p>
      </div>}
    </Dialog>}
  </>;
}

export function Installation() {
  const { t } = useTranslation();
  const query = useQuery(installationRecordsQuery);
  return <section aria-label={t('installationRecords.title')} className="flex min-w-0 flex-col gap-6" aria-busy={query.isFetching}>
    {query.isPending ? <SkeletonRows rows={5} /> : query.isError ? <div className="flex flex-col gap-2">
      {query.dataUpdatedAt > 0 && <p className="text-sm text-text-muted">{t('installationRecords.lastRead')}{' '}<RelativeTime at={new Date(query.dataUpdatedAt).toISOString()} /></p>}
      <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} action={<Button size="sm" pending={query.isRefetching} onClick={() => void query.refetch()}>{t('common.tryAgain')}</Button>} />
    </div> : <RecordSections records={query.data} refreshing={query.isFetching} />}
  </section>;
}
