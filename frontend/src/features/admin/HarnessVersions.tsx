/** Which build of each agent program new and updated environments get, and how far each build is known to work.
 *
 * One compact row for each harness kind: the build Orch ships (as shipped), the newest whose check passed, the
 * newest (untested), or an exact version. It is the system's `agents.harness_versions`; an org may set its own, and
 * an agent profile that names its version keeps it. A new build reaches an environment only by its update.
 */
import { useTranslation } from 'react-i18next';
import { useEffect, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { harnessChannelsKey } from '@/api/admin';

import type { HarnessBuild, HarnessChannelsStatus } from '@/generated/api-types';
import { SettingChangeStatus, useSettingChange, useSystemSettings } from '@/features/admin/SettingRows';
import { BuildStatus } from '@/features/resources/agents/BuildStatus';
import { harnessUsageQuery, programBuildsQuery } from '@/api/harnesses';
import { VersionChoiceControl, resolvedBuild, versionChoice, type VersionChoice } from '@/features/settings/HarnessVersionsChoice';
import { UpdateDialog, type UpdateTargets } from '@/features/fleet/UpdateDialog';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { Dialog } from '@/ui/Dialog';
import { Table } from '@/ui/Table';
import { SkeletonRows } from '@/ui/Skeleton';
import { formatNumber } from '@/lib/format';
import { useFormatContext } from '@/lib/appearance';
import { HelpHint } from '@/ui/HelpHint';
import { useHelpOpener } from '@/features/help/open';
import { Tooltip } from '@/ui/Tooltip';
import { StateLine } from '@/ui/StateLine';
import { RelativeTime } from '@/ui/RelativeTime';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { updateUnits } from '@/features/fleet/updateWords';

const KEY = 'agents.harness_versions';

export { BuildStatus } from '@/features/resources/agents/BuildStatus';

/** The versions guide belongs to the Builds heading, alongside the other section hints. */
export function ProgramVersionsHelp() {
  const { t } = useTranslation();
  const openHelp = useHelpOpener();
  return <HelpHint term={t('programVersions.title')} what={t('programVersions.help')} more={{ label: t('help.hint.more'), onOpen: () => openHelp('agent-program-versions') }} guide={{ article: 'agent-program-versions', heading: null }} />;
}

/** One quiet line says when channels were read and why their schedule is or is not active. */
export function ChannelReadStatus({ value, title, failed = false, error, onRetry }: { value: HarnessChannelsStatus | undefined; title?: (kind: string) => string; failed?: boolean; error?: unknown; onRetry?: () => void }) {
  const { t } = useTranslation();
  if (value === undefined) return failed ? <ErrorAlert error={error} message={t('admin.harnesses.channels.unavailable')} action={onRetry === undefined ? undefined : <Button size="sm" variant="ghost" onClick={onRetry}>{t('common.tryAgain')}</Button>} /> : <StateLine compact role="status" className="text-text-muted" message={t('admin.harnesses.channels.loading')} />;
  const kinds = value.requested.map((kind) => title?.(kind) ?? t('resources.harnessNameUnknown')).join(' · ');
  const reason = t(`admin.harnesses.channels.${value.reason}`, { kinds });
  return <StateLine compact role="status" className="text-text-muted tabular-nums" message={<>
    {value.last_read_at !== null ? <>{t('admin.harnesses.channels.lastRead')} <RelativeTime at={value.last_read_at} /> · </> : value.reason === 'newer' ? <>{t('admin.harnesses.channels.neverRead')} · </> : null}{reason}
  </>} />;
}

/** The version rows, one for each kind that has builds. */
export function HarnessVersions({ kinds, builds, title, catalogue, knownBuilds, selectedBuilds, onFetch, onFollow, onPolicyChange }: { kinds: readonly string[]; builds: readonly HarnessBuild[]; title: (kind: string) => string; catalogue?: ((kind: string) => unknown) | undefined; knownBuilds?: readonly HarnessBuild[]; selectedBuilds?: readonly HarnessBuild[]; onFetch?: (version: string) => Promise<HarnessBuild[]>; onFollow?: () => void; onPolicyChange?: () => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const { find } = useSystemSettings();
  const queryClient = useQueryClient();
  const catalogueBuilds = useQuery({ ...programBuildsQuery, enabled: knownBuilds === undefined, initialData: [...builds] });
  const known = { data: knownBuilds ?? catalogueBuilds.data };
  const usage = useQuery(harnessUsageQuery);
  const [showing, setShowing] = useState<{ kind: string; version: string } | null>(null);
  const [updating, setUpdating] = useState<UpdateTargets | null>(null);
  const change = useSettingChange(KEY);
  const setting = find(KEY);
  const savedSelection = JSON.stringify(setting?.value ?? {});
  useEffect(() => { void queryClient.invalidateQueries({ queryKey: harnessChannelsKey, exact: true }); }, [queryClient, savedSelection]);
  useEffect(() => { void queryClient.invalidateQueries({ queryKey: harnessUsageQuery.queryKey, exact: true }); }, [queryClient, savedSelection, known.data]);
  useEffect(() => { onPolicyChange?.(); }, [savedSelection, onPolicyChange]);
  const current = (change.save.variables as { value?: unknown } | undefined)?.value ?? setting?.value;
  const write = (kind: string, next: VersionChoice) => {
    const all = typeof current === 'object' && current !== null && !Array.isArray(current) ? { ...(current as Record<string, unknown>) } : {};
    // Shipped is what a kind gets when the setting names nothing for it.
    const rest = Object.fromEntries(Object.entries(all).filter(([named]) => named !== kind));
    change.save.mutate({ value: next.use === 'shipped' ? rest : { ...rest, [kind]: next } });
  };
  return (
    <div className="flex flex-col gap-2" data-harness-versions>
      {kinds.map((kind) => {
        const choice = versionChoice(current, kind);
        const platforms = [...new Set(known.data.filter((build) => build.kind === kind).map((build) => `${build.os}/${build.arch}`))];
        const resolved = selectedBuilds?.filter((build) => build.kind === kind) ?? platforms.flatMap((platform) => {
          const build = resolvedBuild(known.data.filter((build) => `${build.os}/${build.arch}` === platform), kind, choice);
          return build === undefined ? [] : [build];
        });
        const used = (usage.data ?? []).filter((row) => row.kind === kind);
        const environments = [...new Set(used.flatMap((row) => row.update_environments))];
        const providers = [...new Set(used.flatMap((row) => row.update_providers))];
        return (
          <div key={kind} className="flex flex-wrap items-end gap-2" data-harness-version={kind}>
            <VersionChoiceControl kind={kind} label={title(kind)} catalogue={catalogue?.(kind)} choice={choice} builds={known.data} target={{ kind: 'system' }} disabled={!change.writable} onFetch={onFetch} onFollow={onFollow} onChange={(next) => write(kind, next)} />
            <span className="flex min-w-0 flex-wrap items-center gap-2 text-sm" aria-label={t('programVersions.resolved', { name: title(kind) })}>{resolved.length === 0 ? t('programVersions.unavailable') : resolved.map((build) => <span key={`${build.os}/${build.arch}`} className="flex min-w-0 flex-wrap items-center gap-2"><span className="min-w-0 wrap-anywhere font-mono">{build.version}</span>{platforms.length > 1 && <span className="text-text-muted">{build.os}/{build.arch}</span>}<BuildStatus build={build} name={title(kind)} catalogue={catalogue?.(kind)} /></span>)}</span>
            {usage.isPending ? <SkeletonRows rows={1} /> : used.map((row) => {
              const environments=t('programVersions.environmentCount',{count:row.environments.length,number:formatNumber(row.environments.length,locale)});
              const providers=t('programVersions.runtimeCount',{count:row.providers.length,number:formatNumber(row.providers.length,locale)});
              const label=t('programVersions.usage',{version:row.version,environments,providers});
              return <Tooltip key={row.version} content={label}><Button size="sm" variant="ghost" className="min-w-0 max-w-full" aria-label={label} onClick={() => setShowing({ kind, version: row.version })}>
                <span className="min-w-0 max-w-(--field-narrow) truncate">{row.version}</span><span className="min-w-0 wrap-anywhere whitespace-normal text-left">· {environments} · {providers}</span>
              </Button></Tooltip>;
            })}
            {environments.length + providers.length > 0 && <Button size="sm" disabled={change.pending} onClick={() => setUpdating({ environments, providers })}>{t('programVersions.updateAvailable', { units: updateUnits(environments.length, providers.length, locale, t) })}</Button>}
          </div>
        );
      })}
      {usage.error !== null && <ErrorAlert error={usage.error} />}
      <SettingChangeStatus change={change} />
      <Dialog open={showing !== null} onOpenChange={(open) => !open && setShowing(null)} title={showing === null ? t('programVersions.running') : <span className="wrap-anywhere">{title(showing.kind)} {showing.version}</span>} footer={<Button onClick={() => setShowing(null)}>{t('common.close')}</Button>}>
        <Table label={t('programVersions.running')} rows={(usage.data ?? []).filter((row) => showing !== null && row.kind === showing.kind && row.version === showing.version).flatMap((row) => [...row.environments.map((environment) => ({ id: environment.id, subject: { kind: 'environment' as const, environment } })), ...row.providers.map((provider) => ({ id: provider.id, subject: { kind: 'provider' as const, provider } }))])} rowKey={(row) => row.id} columns={[{ id: 'unit', header: t('programVersions.unit'), cell: (row) => <ResourceChip subject={row.subject} /> }]} />
      </Dialog>
      {updating !== null && <UpdateDialog targets={updating} onClose={() => setUpdating(null)} />}
    </div>
  );
}
