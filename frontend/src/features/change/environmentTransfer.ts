/** Lifecycle facts belong to the admitted native review, never today's environment placement. */
import type { TFunction } from 'i18next';
import type { EnvironmentTransferKind } from '@/generated/api-types';

export interface TransferFacts { kind: EnvironmentTransferKind | null; sourceHasUnit: boolean | null }
export function transferFacts(value: unknown): TransferFacts {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return { kind: null, sourceHasUnit: null };
  const kind = 'kind' in value && (value.kind === 'move' || value.kind === 'switch_copy') ? value.kind : null;
  const sourceHasUnit = 'source_has_unit' in value && typeof value.source_has_unit === 'boolean' ? value.source_has_unit : null;
  return { kind, sourceHasUnit };
}

export function transferCleanup(phase: string, facts: TransferFacts | undefined, t: TFunction): string | null {
  if (phase !== 'cleanup') return null;
  return t(facts?.sourceHasUnit === true ? 'envTransfer.cleanupUnit' : facts?.sourceHasUnit === false ? 'envTransfer.cleanupSource' : 'envTransfer.cleanup');
}

export function transferWords(facts: TransferFacts, name: string, destination: string | null, t: TFunction) {
  const copy = facts.kind === 'switch_copy';
  return {
    title: t(copy ? 'envTransfer.copyTitle' : facts.kind === 'move' ? 'change.env.title.move' : 'envTransfer.title', { name }),
    heading: destination === null || facts.kind === null ? t('envTransfer.changing', { name })
      : copy ? t('envTransfer.copyHeading', { name, destination }) : t('change.env.heading.move', { name, host: destination }),
    done: destination === null || facts.kind === null ? t('envTransfer.finished', { name })
      : copy ? t('envTransfer.copyDone', { name, destination }) : facts.sourceHasUnit === true ? t('change.env.doneMove', { name, host: destination }) : t('envTransfer.done', { name, destination }),
    confirm: t(copy ? 'envTransfer.copy' : facts.kind === 'move' ? 'change.env.confirm.move' : 'common.continue'),
  };
}
