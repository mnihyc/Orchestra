/** The existing build check, offered at an unavailable Auto choice to a current system administrator. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { checkHarnessBuild } from '@/api/admin';
import { adminModeQuery } from '@/api/adminMode';
import { meQuery } from '@/api/auth';
import { client } from '@/api/client';
import { programBuildsQuery } from '@/api/harnesses';
import type { ApprovalCheckTarget, ResolvedSetting, ThreadParameters } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { ReadFeedback } from '@/ui/ReadFeedback';

const readerOf = () => {
  const state = client.session.getState();
  return JSON.stringify([state.status, state.userId, state.readerVersion]);
};

export function AutoApprovalCheckAction({ target, source, refresh }: { target: ApprovalCheckTarget; source: readonly unknown[]; refresh: readonly (readonly unknown[])[] }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const reader = useStore(client.session, (state) => JSON.stringify([state.status, state.userId, state.readerVersion]));
  const scope = JSON.stringify([reader, target.kind, target.version]);
  const active = useRef<string | null>(scope);
  useLayoutEffect(() => { active.current = scope; return () => { active.current = null; }; }, [scope]);
  // These shell facts already exist; opening an approval choice does not ask for administrator reach.
  const me = useQuery({ ...meQuery, enabled: false }).data;
  const mode = useQuery({ ...adminModeQuery, enabled: false }).data;
  const allowed = me?.is_system_admin === true && mode?.as_admin === true;
  const currentTarget = () => {
    const held = cache.getQueryData<ThreadParameters | ResolvedSetting[]>(source);
    const choices = Array.isArray(held) ? held.find((row) => row.key === 'approval.mode')?.approval_choices : held?.approval_choices;
    return choices?.some((choice) => !choice.available && choice.check?.kind === target.kind && choice.check.version === target.version) === true;
  };
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
    },
  });
  const checking = builds.data?.some((build) => build.kind === target.kind && build.version === target.version && build.status === 'checking') === true;
  useEffect(() => {
    if (!follows || builds.data === undefined || checking || completed.current === scope) return;
    completed.current = scope;
    const finished = () => { if (active.current === scope) setFollowing(null); };
    void Promise.all(refresh.map((queryKey) => cache.invalidateQueries({ queryKey, exact: true }))).then(finished, finished);
  }, [follows, builds.data, checking, refresh, cache, scope]);
  if (!allowed) return null;
  return <span className="inline-flex min-w-0 flex-wrap items-center gap-1" data-auto-check>
    <Button size="sm" variant="ghost" pending={check.isPending && check.variables.scope === scope || follows && checking} onClick={() => check.mutate({ owner: reader, scope })}>{t('admin.harnesses.check')}</Button>
    {check.variables?.scope === scope && check.error !== null && <ErrorAlert error={check.error} />}
    {follows && <ReadFeedback reads={[builds]} pending={builds.isPending} />}
  </span>;
}
