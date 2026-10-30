/** The sessions the harnesses of a machine or of a provider already hold, each with Adopt…, and once adopted the thread it became.
 *
 * A harness is a provider's sign-in run by an agent profile. Each one that can say what it holds lists its
 * sessions, the newest first: the name a session has, or its first words; the folder it ran in; when it last
 * changed; and, for one that is a thread already, the way to it. Adopt… opens the dialog that chooses the topic
 * and says what would happen before anything does. A harness that cannot list what it holds says so, and one
 * whose machine is away says that.
 */
import { useQueries, useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { harnessSessionsQuery } from '@/api/adoption';
import { ApiError } from '@/api/client';
import { placePath } from '@/features/navigation/places';
import { AdoptDialog, sessionName } from '@/features/resources/adopt/AdoptDialog';
import { PathValue } from '@/features/resources/TechnicalValue';
import type { AgentRef, Machine, NativeSession, Provider } from '@/generated/api-types';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { ACTIONS } from '@/ui/actions';
import { StatusDot } from '@/ui/StatusDot';

/** A harness whose sessions are listed: a provider's sign-in, run by an agent profile. */
export interface SessionHarness {
  providerId: string;
  providerName: string;
  agent: AgentRef;
}

/** Observe Activity's existing readers without starting another session request. Model-call
 * readiness is independent of whether an agent can read its saved conversations.
 */
export function ProviderActivityStatus({ harnesses }: { harnesses: readonly SessionHarness[] }) {
  const { t } = useTranslation();
  const reads = useQueries({ queries: harnesses.map((harness) => ({ ...harnessSessionsQuery(harness.providerId, harness.agent.id), enabled: false, throwOnError: false })) });
  if (reads.length === 0) return null;
  if (reads.some((read) => read.isFetching)) return <StatusDot tone="info" label={t('adopt.historyLoading')} showLabel />;
  if (reads.some((read) => read.isError)) return <StatusDot tone="warning" label={t(reads.some((read) => read.isSuccess) ? 'adopt.historyPartial' : 'adopt.historyUnavailable')} showLabel />;
  return reads.every((read) => read.isSuccess) ? <StatusDot tone="idle" label={t('adopt.historyAvailable')} showLabel /> : null;
}

/** The harnesses of a machine: each provider registered from it with each profile of the same harness there. */
export function harnessesOfMachine(machine: Pick<Machine, 'providers' | 'agents'>): SessionHarness[] {
  return machine.providers.flatMap((provider) =>
    provider.harness_kind === null
      ? []
      : machine.agents
          .filter((agent) => agent.harness_kind === provider.harness_kind && !agent.disabled)
          .map((agent) => ({ providerId: provider.id, providerName: provider.name, agent })),
  );
}

/** The harnesses of a provider: the profile its sign-in is held by, and each other that runs on it. */
export function harnessesOfProvider(provider: Pick<Provider, 'id' | 'name' | 'agent' | 'harnesses'>): SessionHarness[] {
  const agents = new Map<string, AgentRef>();
  if (provider.agent !== null) agents.set(provider.agent.id, provider.agent);
  for (const harness of provider.harnesses) agents.set(harness.agent.id, harness.agent);
  return [...agents.values()].map((agent) => ({ providerId: provider.id, providerName: provider.name, agent }));
}

function SessionRow({ session, onAdopt }: { session: NativeSession; onAdopt: () => void }) {
  const { t } = useTranslation();
  const name = sessionName(session, t('adopt.untitled'));
  return (
    <li className="flex flex-wrap items-center gap-x-3 gap-y-1 px-3 py-2">
      <div className="flex min-w-0 flex-1 flex-col">
        <span className="truncate font-medium text-text" title={name}>
          {name}
        </span>
        <span className="flex flex-wrap items-baseline gap-x-2 text-sm text-text-muted">
          {session.cwd === null ? <span>{t('adopt.noFolder')}</span> : <PathValue path={session.cwd} />}
          {session.updated_at !== null && (
            <span>
              {t('adopt.changed')} <RelativeTime at={session.updated_at} />
            </span>
          )}
        </span>
      </div>
      {session.adopted_thread_id !== null ? (
        <Link
          to={placePath({ id: session.adopted_thread_id, kind: 'thread' })}
          aria-label={t(ACTIONS.openThread.named, { name })}
          className="flex items-center gap-1 text-sm font-medium text-accent-text hover:underline"
        >
          <ACTIONS.openThread.Icon aria-hidden /> {t(ACTIONS.openThread.label)}
        </Link>
      ) : (
        <Button size="sm" icon={<ACTIONS.adoptSession.Icon aria-hidden />} onClick={onAdopt} aria-label={t(ACTIONS.adoptSession.named, { name })}>
          {t(ACTIONS.adoptSession.label)}
        </Button>
      )}
    </li>
  );
}

function HarnessSessionList({ harness, showProvider }: { harness: SessionHarness; showProvider: boolean }) {
  const { t } = useTranslation();
  const query = useQuery({ ...harnessSessionsQuery(harness.providerId, harness.agent.id), throwOnError: false });
  const [adopting, setAdopting] = useState<NativeSession | null>(null);
  const id = `sessions-${harness.providerId}-${harness.agent.id}`;
  const title = showProvider ? t('adopt.harness', { agent: harness.agent.name, provider: harness.providerName }) : harness.agent.name;

  let body: React.ReactNode;
  if (query.isPending) {
    body = <SkeletonRows rows={3} />;
  } else if (query.isError) {
    const code = query.error instanceof ApiError ? query.error.code : null;
    body =
      code === 'capability.unsupported' ? (
        <p className="text-sm text-text-muted">{t('adopt.cannotList')}</p>
      ) : code === 'source.offline' ? (
        <p className="text-sm text-text-muted">{t('adopt.offline')}</p>
      ) : (
        <ErrorState compact error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />
      );
  } else if (query.data.length === 0) {
    body = <EmptyState compact title={t('adopt.none')} />;
  } else {
    body = (
      <ul aria-label={title} className="flex flex-col divide-y divide-border rounded-lg border border-border bg-surface">
        {query.data.map((session) => (
          <SessionRow key={session.native_session_ref} session={session} onAdopt={() => setAdopting(session)} />
        ))}
      </ul>
    );
  }

  return (
    <Section id={id} level={3} title={title}>
      {body}
      {adopting !== null && (
        <AdoptDialog session={adopting} providerId={harness.providerId} agentId={harness.agent.id} onClose={() => setAdopting(null)} />
      )}
    </Section>
  );
}

/** The sessions of each harness given, under one heading. */
export function HarnessSessions({ harnesses, showProvider = true }: { harnesses: readonly SessionHarness[]; showProvider?: boolean }) {
  const { t } = useTranslation();
  return (
    <Section id="harness-sessions" title={t('adopt.title')} lead={t('adopt.lead')}>
      {harnesses.length === 0 ? (
        <EmptyState compact title={t('adopt.noHarness')} />
      ) : (
        harnesses.map((harness) => <HarnessSessionList key={`${harness.providerId}-${harness.agent.id}`} harness={harness} showProvider={showProvider} />)
      )}
    </Section>
  );
}
