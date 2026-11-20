/** Conditional note proposals and retained decisions share the subject's existing disclosure. */
import type { TFunction } from 'i18next';
import { useTranslation } from 'react-i18next';
import type { ForkPlanMemory, MemoryOutcome } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { FactDisclosure, type FactGroup } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';

export function MemoryPlanFacts({ memory }: { memory: ForkPlanMemory }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const proposal = memory.proposal;
  const directory = (value: string | null) => value === null ? t('placeFacts.notShared') : <code className="max-w-full wrap-anywhere text-sm">{value}</code>;
  return <FactDisclosure fragment={proposal === null || proposal === undefined ? undefined : t(`placeFacts.notesFates.${proposal.proposed_fate}`)}
    groups={[{ key: 'notes-proposal', title: t('placeFacts.proposal'), facts: [
      { key: 'from', label: t('placeFacts.fromDirectory'), value: proposal === null || proposal === undefined ? null : directory(proposal.source_directory) },
      { key: 'to', label: t('placeFacts.toDirectory'), value: proposal === null || proposal === undefined ? null : directory(proposal.target_directory) },
      { key: 'fate', label: t('placeFacts.notesOutcome'), value: proposal === null || proposal === undefined ? null : t(`placeFacts.notesFates.${proposal.proposed_fate}`) },
      { key: 'revision', label: t('placeFacts.observedRevision'), value: proposal?.observed_target_revision === null || proposal?.observed_target_revision === undefined ? null : formatNumber(proposal.observed_target_revision, locale) },
    ] }]} />;
}
export function memoryResultGroup(outcome: MemoryOutcome | null | undefined, t: TFunction): FactGroup {
  return { key: 'notes-outcome', kind: 'recorded_outcome', title: t('fork.notes.title'), facts: [
    { key: 'fate', label: t('placeFacts.notesOutcome'), value: outcome === null || outcome === undefined ? null : t(`placeFacts.notesFates.${outcome.fate}`) },
    { key: 'at', label: t('placeFacts.recorded'), value: outcome === null || outcome === undefined ? null : <RelativeTime at={outcome.at} /> },
    ...(outcome?.cleanup_pending === true ? [{ key: 'cleanup', label: t('placeFacts.cleanup'), value: t('placeFacts.copyCleanupPending') }] : []),
  ] };
}
export function MemoryResultFacts({ outcome }: { outcome: MemoryOutcome | null | undefined }) {
  const { t } = useTranslation();
  return <FactDisclosure fragment={outcome?.cleanup_pending === true ? t('placeFacts.copyCleanupPending') : undefined} groups={[memoryResultGroup(outcome, t)]} />;
}
