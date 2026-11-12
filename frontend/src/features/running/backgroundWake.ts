/** Recorded background identities supply labels; a transport outage never invents a lost task. */
import type { TFunction } from 'i18next';
import type { BackgroundTask, BackgroundWakeStatus, BackgroundWork } from '@/generated/api-types';

export function backgroundName(task: Pick<BackgroundWork, 'kind' | 'label'>, t: TFunction): string {
  return task.label.trim() === '' ? t(task.kind === 'subagent' ? 'backgroundWake.unnamedSubagent' : 'backgroundWake.unnamedCommand') : task.label;
}

export function wakeCauses(causes: readonly BackgroundTask[], t: TFunction): string {
  return causes.filter((task) => task.state === 'completed' || task.state === 'failed')
    .map((task) => t(task.state === 'failed' ? 'backgroundWake.failed' : 'backgroundWake.finished', { label: backgroundName(task, t) })).join(', ');
}

export function backgroundNotice(status: BackgroundWakeStatus, t: TFunction): string | null {
  if (status.tasks.some((task) => task.state === 'running')) return t(status.setting && status.effective ? 'backgroundWake.willWake' : 'backgroundWake.nextInput');
  return status.tasks.some((task) => task.state === 'lost') ? t('backgroundWake.lost') : null;
}
