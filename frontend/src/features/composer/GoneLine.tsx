/** What a thread runs with that is gone, said in one line above the composer's controls.
 *
 * Its provider, its model, its agent profile or that profile's machine was deleted, or its environment removed:
 * the thread keeps its whole history and its message box, and the line names what went, by the name it had.
 * Whoever may change what the thread runs on gets "Choose another": the model list, which holds the models of the
 * other providers, where only the provider or the model went; otherwise the switch, at the environment where that
 * went. Anybody else reads who can change it. The banner above does not say the same thing again.
 */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';

import { openChange } from '@/features/change/target';
import { openWhere } from '@/features/thread/openWhere';
import type { BlockedReason, Thread, ThreadGone, ThreadGoneKind } from '@/generated/api-types';
import { Button } from '@/ui/Button';

/** The reasons a thread is blocked for that a gone reference already explains. */
const EXPLAINS: Readonly<Record<ThreadGoneKind, readonly BlockedReason[]>> = {
  provider: ['provider_unavailable'],
  model: ['model_not_allowed', 'model_not_set'],
  agent: ['agent_disabled'],
  machine: ['agent_offline', 'agent_disabled'],
  environment: ['unbound', 'environment_unavailable'],
};

/** Whether the line says why the thread is blocked, so that its banner yields to it. */
export function goneExplains(gone: readonly ThreadGone[], reason: BlockedReason): boolean {
  return gone.some((entry) => EXPLAINS[entry.kind].includes(reason));
}

/** One gone reference in words, by the name it had. */
export function goneWords(entry: ThreadGone, t: TFunction): string {
  const name = entry.name.trim();
  switch (entry.kind) {
    case 'provider':
      return t('composer.gone.provider', { name });
    case 'model':
      return t('composer.gone.model', { name });
    case 'agent':
      return t('composer.gone.agent', { name });
    case 'machine':
      return name === '' ? t('composer.gone.machineUnnamed') : t('composer.gone.machine', { name });
    case 'environment':
      return name === '' ? t('composer.gone.environmentUnnamed') : t('composer.gone.environment', { name });
  }
}

export interface GoneLineProps {
  thread: Pick<Thread, 'id' | 'name'>;
  gone: readonly ThreadGone[];
  /** Whether this person may change what the thread runs on now. */
  canChange: boolean;
  /** Why not, when they may not. */
  reason?: string | undefined;
  /** Opens the composer's model list. */
  onChooseModel: () => void;
}

export function GoneLine({ thread, gone, canChange, reason, onChooseModel }: GoneLineProps) {
  const { t } = useTranslation();
  if (gone.length === 0) return null;
  const modelOnly = gone.every((entry) => entry.kind === 'provider' || entry.kind === 'model');
  const choose = () => {
    if (modelOnly) onChooseModel();
    else if (gone.some((entry) => entry.kind === 'environment')) openWhere(thread);
    else openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'switch', focus: 'provider' });
  };
  return <div role="group" aria-label={t('composer.gone.label')} data-gone-line className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-text-muted">
    {gone.map((entry) => <span key={`${entry.kind}:${entry.id}:${entry.name}`} data-gone={entry.kind}>{goneWords(entry, t)}</span>)}
    {canChange ? <Button size="sm" variant="ghost" onClick={choose}>{t('composer.gone.choose')}</Button>
      : reason !== undefined && <span>{reason}</span>}
  </div>;
}
