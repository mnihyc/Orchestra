/** Provider, model and effort, the thread's environment with its owner, and the arrangement's type, what the agent works with there and isolation facts.
 *
 * Where the agent works and how it is placed. The environment chip names the environment and where it is,
 * whose it is when that is somebody else, how it stands, and how many other threads work in the same
 * directory. The arrangement badge says in words how harness, credential and tools are placed ("Separated",
 * "Key stays with provider", "Token shared", "Sign-in handed over", "Together"), followed by whose tools the
 * agent works with and, where it has none of Orch's own tools, that; a shared credential, a handover, being
 * together and missing tools are drawn as warnings. Its tooltip says where each part is.
 */
import { Link } from '@tanstack/react-router';
import { TriangleAlert } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import type { Thread } from '@/generated/api-types';
import { cx } from '@/ui/cx';
import { EnvironmentDot } from '@/ui/EnvironmentDot';
import { Tooltip } from '@/ui/Tooltip';
import { UserChip } from '@/ui/UserChip';
import { ResourceHint, ResourceName } from '@/features/resource-facts/ResourceFacts';
import { Badge } from '@/ui/Badge';
import { isolationToolWords } from '@/features/resources/executorNames';

export function EnvironmentChip({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const meId = useStore(client.session, (state) => state.userId);
  const environment = thread.environment;
  if (environment === null) {
    return <span className="text-text-subtle">{t('thread.environment.none')}</span>;
  }
  const others = thread.directory_threads;
  return (
      <span className="inline-flex max-w-(--chip-width-wide) items-center gap-1 rounded-md px-1 text-text-muted" data-environment>
        <EnvironmentDot status={environment.status} />
        <ResourceName subject={{ kind: 'environment', environment }} asChild><Link to="/resources/environments/$id" params={{ id: environment.id }} className="truncate text-text hover:underline">
          {environment.name}
        </Link></ResourceName>
        <ResourceHint subject={{ kind: 'environment', environment }} className="truncate" />
        {environment.owner.id !== meId && (
          <UserChip user={environment.owner} size={24} />
        )}
        {others > 0 && <span className="text-xs text-text-subtle">{t('thread.environment.others', { count: others })}</span>}
      </span>
  );
}

export function IsolationBadge({ thread }: { thread: Thread }) {
  const { t, i18n } = useTranslation();
  const binding = thread.binding;
  if (binding === null) return null;
  const isolation = binding.isolation;
  const tools = isolationToolWords(isolation, t, i18n.resolvedLanguage ?? i18n.language, binding.agent.name);
  const noShared = !isolation.orch_tools.offered;
  const warn = ['credential_shared', 'handed_over', 'together'].includes(isolation.type) || noShared;
  const reason = isolation.orch_tools.reason;
  const why = [
    isolation.harness_runs_in === 'provider' ? t('thread.isolation.harnessProvider') : t('thread.isolation.harnessEnvironment'),
    t(`thread.isolation.credential.${isolation.credential_path}`),
    tools ?? t('thread.isolation.toolsOwn'),
    noShared && reason !== null ? t(`thread.isolation.noShared.${reason}`, { agent: binding.agent.name, environment: thread.environment?.name ?? '', thread: thread.name }) : null,
  ].filter((line): line is string => line !== null);
  return (
    <Tooltip
      content={
        <span className="flex flex-col gap-1">
          {why.map((line) => (
            <span key={line}>{line}</span>
          ))}
        </span>
      }
    >
      <span className="inline-flex flex-wrap items-center gap-1">
      <span className={cx('inline-flex items-center gap-1 rounded-full border px-2 text-xs', warn ? 'border-warning/50 bg-warning-subtle text-text' : 'border-border text-text-muted')} data-isolation={isolation.type}>
        {warn && <TriangleAlert className="text-warning" aria-hidden />}
        {t(`thread.isolation.type.${isolation.type}`)}
      </span>
        {noShared && <Badge tone="warning">{t('thread.isolation.noSharedTools')}</Badge>}
      </span>
    </Tooltip>
  );
}

/** What the thread runs on, in a few lines: for an empty conversation. */
export function BindingSummary({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const binding = thread.binding;
  if (binding === null) return null;
  return (
    <ul className="flex flex-col items-center gap-1 text-sm text-text-muted">
      <li className="inline-flex min-w-0 flex-wrap items-center gap-2">
        <ResourceName subject={{ kind: 'agent', agent: binding.agent }} hint={false} />
        <ResourceName subject={{ kind: 'provider', provider: binding.provider }} />
      </li>
      <li>{thread.environment === null ? t('thread.environment.none') : <ResourceName subject={{ kind: 'environment', environment: thread.environment }} />}</li>
    </ul>
  );
}
