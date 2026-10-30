/** Program policy controls share known versions and an explicit exact-version fetch at every scope. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { BUILD_CHECK_FOLLOW_MS, fetchProgramVersion, programBuildsQuery } from '@/api/harnesses';
import type { SettingsTarget } from '@/api/settings';
import { BuildStatus } from '@/features/resources/agents/BuildStatus';
import { programNameIn } from '@/features/resources/programNames';
import { useHarnessNames } from '@/features/resources/harnesses';
import type { HarnessBuild } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';

import { buildRefusal, compareVersions } from '@/lib/versions';
import { choiceOption } from '@/i18n/choices';
import { ApiError } from '@/api/client';
import { versionChoice, type VersionChoice } from './versionPolicy';

export { PROGRAM_VERSIONS_KEY, versionChoice, type VersionChoice, type VersionPolicy } from './versionPolicy';

export function resolvedBuild(builds: readonly HarnessBuild[], kind: string, choice: VersionChoice): HarnessBuild | undefined {
  const available = builds.filter((build) => build.kind === kind).sort((a, b) => compareVersions(b.version, a.version));
  return choice.use === 'exact' ? available.find((build) => build.version === choice.version) : choice.use === 'newest' ? available[0] : choice.use === 'newest_checked' ? available.find((build) => build.status === 'checked') ?? available.find((build) => build.is_default) : available.find((build) => build.is_default);
}

export function VersionChoiceControl({ kind, choice, builds, disabled = false, target, onChange, onApply, onEdited, label, catalogue, inherited = false, onInherit, onFetch, onFollow }: { kind: string; choice: VersionChoice; builds: readonly HarnessBuild[]; disabled?: boolean; target: SettingsTarget; onChange: (choice: VersionChoice) => void; onApply?: (choice: VersionChoice) => void; onEdited?: () => void; label?: string; catalogue?: unknown; inherited?: boolean; onInherit?: () => void; onFetch?: (version: string) => Promise<HarnessBuild[]>; onFollow?: () => void }) {
  const { t, i18n } = useTranslation();
  const client = useQueryClient();
  const name = label ?? programNameIn(builds.find((build) => build.kind === kind), i18n.resolvedLanguage ?? i18n.language) ?? t('resources.harnessNameUnknown');
  const id = useId();
  const input = useRef<HTMLInputElement>(null);
  const [exact, setExact] = useState(choice.version ?? '');
  const [pickingExact, setPickingExact] = useState(false);
  const [requested, setRequested] = useState<string | null>(null);
  const fetch = useMutation({ mutationFn: () => onFetch !== undefined ? onFetch(exact.trim()) : fetchProgramVersion(kind, exact.trim(), target.kind === 'environment' ? { environment_id: target.id } : target.kind === 'node' ? { node_id: target.id } : {}),
    onSuccess: (known) => { if (onFetch === undefined) client.setQueryData(programBuildsQuery.queryKey, known); setRequested(exact.trim()); },
  });
  const versions = [...new Set(builds.filter((build) => build.kind === kind).map((build) => build.version))].sort((a, b) => compareVersions(b, a));
  const found = builds.find((build) => build.kind === kind && build.version === exact.trim());
  const exactBuilds = builds.filter((build) => build.kind === kind && build.version === exact.trim());
  const refusal = exactBuilds.map(buildRefusal).find((reason) => reason !== null) ?? null;
  const checking = exactBuilds.some((build) => build.status === 'checking');
  const completed = refusal === null && exactBuilds.length > 0 && exactBuilds.every((build) => build.status !== 'checking' && (build.source !== 'channel' || build.check !== undefined && build.check !== null));
  const fetchReason = fetch.error instanceof ApiError
    ? fetch.error.faults().find((fault) => fault.path === 'version')?.message ?? fetch.error.problem.detail
    : fetch.error instanceof Error ? fetch.error.message : t('programVersions.unavailable');
  const apply = onApply ?? onChange;
  useEffect(() => {
    if (requested === null || completed || refusal !== null) return;
    const follow = setInterval(() => { if (onFollow !== undefined) onFollow(); else void client.invalidateQueries({ queryKey: programBuildsQuery.queryKey, exact: true }); }, BUILD_CHECK_FOLLOW_MS);
    return () => clearInterval(follow);
  }, [client, requested, completed, refusal, onFollow]);
  return <div className="flex min-w-0 flex-wrap items-end gap-2">
    <Select label={name} fieldClassName="w-(--field-wide)" disabled={disabled} value={pickingExact ? 'exact' : inherited ? 'inherit' : choice.use}
      onValueChange={(use) => { if (use === 'inherit') { setPickingExact(false); onInherit?.(); } else if (use === 'exact') { const version = choice.version ?? resolvedBuild(builds, kind, choice)?.version ?? versions[0] ?? ''; setExact(version); setPickingExact(true); } else if (use === 'shipped' || use === 'newest_checked' || use === 'newest') { setPickingExact(false); onChange({ use }); } }}
      options={[...(onInherit === undefined ? [] : [{ value: 'inherit', name: t('programVersions.inherit'), description: t('programVersions.inheritAbout') }]), ...(['shipped', 'newest_checked', 'newest', 'exact'] as const).map((use) => ({ value: use, ...choiceOption(t, `harnessVersion.${use}`) }))]} />
    {(!inherited && choice.use === 'exact' || pickingExact) && <>
      <Input ref={input} required label={t('admin.harnesses.version')} list={id} fieldClassName="w-(--field-narrow)" value={exact} disabled={disabled || fetch.isPending} onChange={(event) => { onEdited?.(); fetch.reset(); setExact(event.target.value); setRequested(null); event.target.setCustomValidity(event.target.value.trim() === choice.version ? '' : t('programVersions.fetchFirst')); }} />
      <datalist id={id}>{versions.map((version) => <option key={version} value={version} />)}</datalist>
      <Button size="sm" disabled={disabled || refusal !== null || exact.trim() === '' || checking || requested === exact.trim() && !completed} pending={fetch.isPending} onClick={() => { if (completed) { input.current?.setCustomValidity(''); setPickingExact(false); apply({ use: 'exact', version: exact.trim() }); } else fetch.mutate(); }}>
        {refusal !== null ? t('programVersions.useVersion') : completed ? t('programVersions.useVersion') : requested === exact.trim() || checking ? t('programVersions.checking') : t('programVersions.fetch')}
      </Button>
      {found !== undefined && <BuildStatus build={found} name={label} catalogue={catalogue} />}
      {refusal !== null && <StateLine compact tone="warning" className="min-w-0 max-w-full basis-full wrap-anywhere" message={t('programVersions.fetchRefused', { name: name, version: exact.trim(), reason: refusal })} />}
      {fetch.error !== null && <ErrorAlert error={fetch.error} message={t('programVersions.fetchRefused', { name: name, version: exact.trim(), reason: fetchReason })} />}
    </>}
  </div>;
}

export function HarnessVersionsChoice({ value, effective, onChange, onApply, disabled, target, onEdited }: { value: unknown; effective?: unknown; onChange: (value: unknown) => void; onApply?: (value: unknown) => void; disabled?: boolean; target: SettingsTarget; onEdited?: () => void }) {
  const { t } = useTranslation();
  const builds = useQuery(programBuildsQuery);
  const programs = useHarnessNames();
  if (builds.isPending) return <SkeletonRows rows={2} />;
  if (builds.isError) return <ErrorAlert error={builds.error} />;
  const current = typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};
  const inherited = typeof effective === 'object' && effective !== null && !Array.isArray(effective) ? effective as Record<string, unknown> : {};
  const kinds = [...new Set([...builds.data.map((build) => build.kind), ...Object.keys(inherited), ...Object.keys(current)])];
  return <div className="flex flex-col gap-2">
    {programs.kinds.isPending && <StateLine compact message={t('programList.loadingNames')} />}
    {programs.kinds.isError && <ErrorState compact error={programs.kinds.error} onRetry={() => void programs.kinds.refetch()} />}
    {kinds.map((kind) => <VersionChoiceControl key={kind} kind={kind} label={programs.title(kind)} choice={versionChoice({ ...inherited, ...current }, kind)} target={target} builds={builds.data} disabled={disabled}
    inherited={target.kind !== 'system' && !(kind in current)} onInherit={target.kind === 'system' ? undefined : () => { onEdited?.(); onChange(Object.fromEntries(Object.entries(current).filter(([key]) => key !== kind))); }}
    onEdited={onEdited} onChange={(choice) => onChange({ ...current, [kind]: choice })} onApply={onApply === undefined ? undefined : (choice) => onApply({ ...current, [kind]: choice })} />)}</div>;
}
