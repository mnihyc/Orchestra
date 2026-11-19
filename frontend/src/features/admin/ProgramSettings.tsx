/** One program's policy retains the shared version, running-use and reviewed update controls. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { adminProgramBuildsQuery, adminProgramQuery, adminProgramVersionQuery, fetchAdminProgramVersion } from '@/api/programAdmin';
import { HarnessVersions } from '@/features/admin/HarnessVersions';
import { useSystemSettings } from '@/features/admin/SettingRows';
import { programName } from '@/features/resources/programNames';
import { PROGRAM_VERSIONS_KEY, versionChoice } from '@/features/settings/versionPolicy';
import type { ProgramDetail } from '@/generated/api-types';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function ProgramSettings({ detail }: { detail: ProgramDetail }) {
  const { t, i18n } = useTranslation();
  const client = useQueryClient();
  const kind = detail.summary.harness.kind;
  const name = programName(detail.summary.harness, i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown');
  const { settings, find } = useSystemSettings();
  const choice = versionChoice(find(PROGRAM_VERSIONS_KEY)?.value ?? detail.versions, kind);
  const [requested, setRequested] = useState<string | null>(null);
  const version = requested ?? (choice.use === 'exact' ? choice.version : undefined);
  const first = useQuery({ ...adminProgramBuildsQuery(kind, undefined), enabled: version === undefined });
  const exact = useQuery({ ...adminProgramVersionQuery(kind, version ?? ''), enabled: version !== undefined });
  const rows = version === undefined ? first.data?.items : exact.data;
  const reloadDetail = useCallback(() => { void client.invalidateQueries({ queryKey: adminProgramQuery(kind).queryKey, exact: true }); }, [client, kind]);
  const follow = useCallback(() => {
    reloadDetail();
    void client.invalidateQueries({ queryKey: version === undefined ? adminProgramBuildsQuery(kind, undefined).queryKey : adminProgramVersionQuery(kind, version).queryKey, exact: true });
  }, [client, kind, version, reloadDetail]);
  if (settings.isPending) return <SkeletonRows rows={2} />;
  if (settings.isError) return <ErrorState compact error={settings.error} onRetry={() => void settings.refetch()} />;
  return <div className="flex min-w-0 flex-col gap-4">
    <HarnessVersions kinds={[kind]} title={() => name} catalogue={() => detail.program?.checks ?? null} builds={rows ?? []} knownBuilds={rows ?? []} selectedBuilds={detail.resolved_builds}
      onFetch={async (version) => { const found = await fetchAdminProgramVersion(kind, version); client.setQueryData(adminProgramVersionQuery(kind, version).queryKey, found); setRequested(version); return found; }}
      onFollow={follow} onPolicyChange={reloadDetail} />
    {(version === undefined ? first.isPending : exact.isPending) && <SkeletonRows rows={1} />}
    {(version === undefined ? first.error : exact.error) !== null && <ErrorAlert error={version === undefined ? first.error : exact.error} />}
  </div>;
}
