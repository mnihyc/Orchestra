/** The existing build check, offered at an unavailable Auto choice to a current system administrator. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { checkHarnessBuild } from '@/api/admin';
import { adminModeQuery } from '@/api/adminMode';
import { meQuery } from '@/api/auth';
import { client } from '@/api/client';
import { checkReason } from '@/features/resources/agents/buildChecks';
import { programBuildsQuery } from '@/api/harnesses';
import type { ApprovalCheckTarget, HarnessBuild, ResolvedSetting, RunningHarnessBuild, ThreadParameters } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { useServerNow } from '@/lib/serverClock';
import { formatSpan, useRunningClock } from '@/lib/time';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { ReadFeedback } from '@/ui/ReadFeedback';

const readerOf = () => {
  const state = client.session.getState();
  return JSON.stringify([state.status, state.userId, state.readerVersion]);
};

/** Where no build host is online to run the check, a current system administrator is led to the hosts. */
export function AutoCheckHostLead() {
  const { t } = useTranslation();
  const me = useQuery({ ...meQuery, enabled: false }).data;
  const mode = useQuery({ ...adminModeQuery, enabled: false }).data;
  if (me?.is_system_admin !== true || mode?.as_admin !== true) return <span className="text-xs text-text-muted">{t('composer.approvalUnavailable.askAdmin')}</span>;
  return <Link to="/admin/$section" params={{ section: 'hosts' }} className="w-fit text-xs font-medium text-accent-text hover:underline" data-auto-check-hosts>{t('admin.sections.hosts.title')}</Link>;
}

/**
 * Auto's check as its build's record has it, beside the choice: one under way says where and for how long, and once it
 * ends without passing, why. Followed while it runs, so that the choice is read again when it ends.
 */
export function AutoCheckProgress({ target, build, refresh }: { target: ApprovalCheckTarget; build?: HarnessBuild | null | undefined; refresh: readonly (readonly unknown[])[] }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const format = useFormatContext();
  const same = (row: HarnessBuild) => row.kind === target.kind && row.version === target.version && row.sha256 === target.sha256;
  // The choice says how the check stands; a check started here has the choice read again at once.
  const listed = build ?? undefined;
  const running = listed?.status === 'checking';
  // Only a list read while this check is followed speaks for it; an older one may hold a check that ended since.
  const fetched = useQuery({ ...programBuildsQuery, enabled: running }).data;
  const followed = (running ? fetched?.find(same) : undefined) ?? listed;
  const watched = useRef(false);
  useEffect(() => {
    if (followed?.status === 'checking') { watched.current = true; return; }
    if (!watched.current) return;
    watched.current = false;
    void Promise.all(refresh.map((queryKey) => cache.invalidateQueries({ queryKey, exact: true })));
  }, [followed?.status, refresh, cache]);
  const deviceNow = useRunningClock(followed?.status === 'checking');
  const now = useServerNow(deviceNow) ?? deviceNow;
  const check = followed?.check;
  if (followed === undefined || check === null || check === undefined) return null;
  if (followed.status === 'checking') {
    const elapsed = formatSpan(Math.max(0, now - Date.parse(check.at)), format);
    return <span role="status" className="text-xs text-text-muted tabular-nums wrap-anywhere" data-auto-check-progress>
      {check.machine === null ? t('composer.approvalUnavailable.checkRunningAnywhere', { elapsed }) : t('composer.approvalUnavailable.checkRunning', { host: check.machine, elapsed })}
    </span>;
  }
  const reason = checkReason(followed, t);
  if (reason === null) return null;
  return <span className="text-xs text-text-muted wrap-anywhere" data-auto-check-progress>
    {t(followed.status === 'failed' ? 'composer.approvalUnavailable.checkFailed' : 'composer.approvalUnavailable.checkCouldNotRun', { reason })}
  </span>;
}

export function AutoApprovalCheckAction({ target, source, refresh, onChecked, installed = false }: { target: ApprovalCheckTarget; source: readonly unknown[]; refresh: readonly (readonly unknown[])[]; onChecked?: (() => void) | undefined; installed?: boolean }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const reader = useStore(client.session, (state) => JSON.stringify([state.status, state.userId, state.readerVersion]));
  const scope = JSON.stringify([reader, target.kind, target.version, target.machine_id, target.sha256]);
  const active = useRef<string | null>(scope);
  useLayoutEffect(() => { active.current = scope; return () => { active.current = null; }; }, [scope]);
  // These shell facts already exist; opening an approval choice does not ask for administrator reach.
  const me = useQuery({ ...meQuery, enabled: false }).data;
  const mode = useQuery({ ...adminModeQuery, enabled: false }).data;
  const allowed = me?.is_system_admin === true && mode?.as_admin === true;
  const currentTarget = useCallback(() => {
    if (installed) return cache.getQueryData<RunningHarnessBuild[]>(source)?.some((program) =>
      program.kind === target.kind && program.version === target.version && program.check_machine_id === target.machine_id && program.build?.sha256 === target.sha256) === true;
    const held = cache.getQueryData<ThreadParameters | ResolvedSetting[]>(source);
    const choices = Array.isArray(held) ? held.find((row) => row.key === 'approval.mode')?.approval_choices : held?.approval_choices;
    return choices?.some((choice) => !choice.available && choice.check?.kind === target.kind && choice.check.version === target.version && choice.check.machine_id === target.machine_id && choice.check.sha256 === target.sha256) === true;
  }, [cache, source, installed, target.kind, target.version, target.machine_id, target.sha256]);
  const [following, setFollowing] = useState<string | null>(null);
  const completed = useRef<string | null>(null);
  const follows = allowed && following === scope;
  const builds = useQuery({ ...programBuildsQuery, enabled: follows });
  const check = useMutation({
    mutationFn: async (request: { owner: string; scope: string }) => {
      if (request.owner !== readerOf() || request.scope !== active.current || !currentTarget()) return { ...request, result: null };
      const result = await checkHarnessBuild(target);
      return { ...request, result };
    },
    onSuccess: ({ owner, scope, result }) => {
      if (result === null || owner !== readerOf() || scope !== active.current || !currentTarget()) return;
      cache.setQueryData(programBuildsQuery.queryKey, result);
      completed.current = null;
      setFollowing(scope);
      // A check that runs for minutes is shown with the choice from its start, wherever the choice is read next.
      if (result.some((row) => row.kind === target.kind && row.version === target.version && row.sha256 === target.sha256 && row.status === 'checking')) {
        void Promise.all(refresh.map((queryKey) => cache.invalidateQueries({ queryKey, exact: true })));
      }
    },
  });
  const build = builds.data?.find((row) => row.kind === target.kind && row.version === target.version && row.sha256 === target.sha256);
  const checking = build?.status === 'checking';
  useEffect(() => {
    if (!follows || builds.data === undefined || checking || completed.current === scope || !currentTarget()) return;
    completed.current = scope;
    onChecked?.();
    const finished = () => { if (active.current === scope) setFollowing(null); };
    void Promise.all(refresh.map((queryKey) => cache.invalidateQueries({ queryKey, exact: true }))).then(finished, finished);
  }, [follows, builds.data, checking, refresh, cache, scope, currentTarget, onChecked]);
  if (!allowed) return <span className="text-xs text-text-muted">{t('composer.approvalUnavailable.askAdmin')}</span>;
  return <span className="inline-flex min-w-0 flex-wrap items-center gap-1" data-auto-check>
    <Button size="sm" variant="ghost" pending={check.isPending && check.variables.scope === scope || follows && checking} onClick={() => check.mutate({ owner: reader, scope })}>{t('admin.harnesses.check')}</Button>
    {check.variables?.scope === scope && check.error !== null && <ErrorAlert error={check.error} />}
    {follows && <ReadFeedback reads={[builds]} pending={builds.isPending} />}
  </span>;
}
