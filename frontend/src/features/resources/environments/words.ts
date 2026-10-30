/** An environment in a few words: how it runs ("From Default on host-2", "~/work on Alice's laptop"), how it stands, and what the reader may do there.
 */
import type { TFunction } from 'i18next';

import type { Environment, EnvironmentRemovalPhase, EnvironmentStatus } from '@/generated/api-types';
import { record } from '@/features/resources/UnitCommands';

/** Every unit can report a machine; directory attachment is a lifecycle fact, not that reference. */
export function environmentLifecycle(environment: Pick<Environment, 'facts' | 'template'>): 'unit' | 'attach' | null {
  const lifecycle = record(record(environment.facts).environment_descriptor).lifecycle;
  if (lifecycle === 'unit' || lifecycle === 'attach') return lifecycle;
  return environment.template === null ? null : 'unit';
}

/** Directory facts show their last two names; an absolute account path belongs in hover or Copy path. */
export function shortRoot(root: string): string {
  const parts = root.split(/[\\/]/u).filter((part) => part !== '');
  return parts.length > 2 ? `…/${parts.slice(-2).join('/')}` : parts.join('/') || root;
}

/** How an environment runs: the directory on the machine it is, or the template it was made from and where. A list gives `short`, and its cell holds the whole as a title. */
export function howItRuns(environment: Pick<Environment, 'machine' | 'template' | 'host' | 'workspace_root'>, t: TFunction, short = false): string {
  if (environment.machine !== null) return t('environments.how.machine', { root: short ? shortRoot(environment.workspace_root) : environment.workspace_root, machine: environment.machine.name });
  if (environment.template !== null) {
    return environment.host === null
      ? t('environments.how.template', { template: environment.template.name })
      : t('environments.how.templateOn', { template: environment.template.name, host: environment.host.name });
  }
  return t('environments.how.none');
}

/** What the reader may do in an environment, as the highest of what they hold. */
export type Holding = 'manage' | 'use' | 'view';

export function holding(permissions: readonly string[]): Holding {
  if (permissions.includes('environment.manage')) return 'manage';
  if (permissions.includes('environment.use')) return 'use';
  return 'view';
}

/** How many threads an environment holds that the reader may see, with the others, and in how many topics. */
export function threadCounts(environment: Pick<Environment, 'threads' | 'threads_hidden'>): { threads: number; topics: number; hidden: number } {
  return {
    threads: environment.threads.length,
    topics: new Set(environment.threads.map((entry) => entry.topic.id)).size,
    hidden: environment.threads_hidden,
  };
}

/** Whether a status says that a job is under way on the environment. */
export const isBusy = (status: EnvironmentStatus): boolean => status === 'provisioning' || status === 'recreating' || status === 'moving';

/** Old and new provisioning diagnostics use the same actionable words instead of repeating their image phase. */
export function environmentStatusDetail(environment: Pick<Environment, 'status' | 'status_detail'>, t: TFunction): string | null {
  const detail = environment.status_detail;
  return environment.status === 'failed' && detail !== null && /^image:\s*/u.test(detail)
    ? t('environments.overview.imageFailed', { reason: detail.replace(/^(?:image:\s*)+/u, '') }) : detail;
}

/** An accepted lifecycle job precedes the next physical-status report from its unit. */
export function environmentDisplayStatus(environment: Pick<Environment, 'status'> & Partial<Pick<Environment, 'job'>>): EnvironmentStatus {
  const job = environment.job;
  if (job?.state !== 'queued' && job?.state !== 'running') return environment.status;
  switch (job.kind) {
    case 'environment_provision': return 'provisioning';
    case 'environment_recreate': return 'recreating';
    case 'environment_move': return 'moving';
    default: return environment.status;
  }
}

export function environmentRemovalPhase(environment: Partial<Pick<Environment, 'removal' | 'job'>>): EnvironmentRemovalPhase | undefined {
  const phase = environment.removal?.phase;
  if (phase !== undefined) return phase;
  const job = environment.job;
  if (job?.kind !== 'environment_remove' || job.state !== 'queued' && job.state !== 'running') return undefined;
  return job.state === 'queued' ? 'scheduled' : 'removing';
}

/** Replacement and accepted removal describe work in progress, not an installed unit behind its desired definition. */
export function environmentChangesVisible(environment: Pick<Environment, 'status'> & Partial<Pick<Environment, 'removal' | 'job'>>): boolean {
  const status = environmentDisplayStatus(environment);
  return status !== 'removed' && !isBusy(status) && environmentRemovalPhase(environment) === undefined;
}

/** How often a screen asks again while a job works on an environment: often enough to see it finish, seldom enough to cost nothing. */
export const BUSY_POLL_MS = 2_000;
