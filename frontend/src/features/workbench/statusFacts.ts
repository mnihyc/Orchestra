/** Human words for thread status facts, including observations whose protocol reason must stay out of the screen. */
import type { TFunction } from 'i18next';
import type { CapStatus, GitStatus, Thread } from '@/generated/api-types';
import { isolationToolWords } from '@/features/resources/executorNames';
import { currentLocale } from '@/i18n';
import { formatDateTime, type TimeContext } from '@/lib/time';

/**
 * A repository's branch for the status bar, with when it was checked; nothing for anything else.
 *
 * Nothing in a thread needs Git, so a plain directory is not a warning, and a status that could not
 * be read is said in the Git view, where Git is offered, rather than in the bar.
 */
export function workspaceWords(thread: Pick<Thread, 'attention'>, git: GitStatus | undefined, t: TFunction, format: TimeContext): { label: string; hint: string } | null {
  if (git?.available !== true) return null;
  const label = git.branch ?? t('inspector.git.detached');
  const at = thread.attention.as_of;
  const hint = at === null || !Number.isFinite(Date.parse(at)) ? label : t('workbench.status.checked', { fact: label, time: formatDateTime(at, format) });
  return { label, hint };
}

export function isolationWords(thread: Pick<Thread, 'binding' | 'environment' | 'name'>, t: TFunction, locale = currentLocale()): { label: string; hint: string; warning: boolean } | null {
  const binding = thread.binding;
  if (binding === null) return null;
  const isolation = binding.isolation;
  const tools = isolationToolWords(isolation, t, locale, binding.agent.name);
  const noShared = !isolation.orch_tools.offered;
  const hint = [
    isolation.harness_runs_in === 'provider' ? t('thread.isolation.harnessProvider') : t('thread.isolation.harnessEnvironment'),
    t(`thread.isolation.credential.${isolation.credential_path}`),
    tools ?? t('thread.isolation.toolsOwn'),
    noShared && isolation.orch_tools.reason !== null ? t(`thread.isolation.noShared.${isolation.orch_tools.reason}`, { agent: binding.agent.name, environment: thread.environment?.name ?? '', thread: thread.name }) : null,
  ].filter((line): line is string => line !== null).join(' ');
  return { label: t(`thread.isolation.type.${isolation.type}`),
    hint, warning: ['credential_shared', 'handed_over', 'together'].includes(isolation.type) || noShared };
}

/** The server classifies a near cap; use that classification rather than inventing another threshold. */
export function nearLimit(caps: readonly CapStatus[]): CapStatus | null {
  const relevant = caps.filter((status) => status.cap.enabled && status.replaced_by === null && status.state !== 'ok');
  return relevant.find((status) => status.state === 'reached') ?? relevant[0] ?? null;
}
