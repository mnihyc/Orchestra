/** The report's captured helper source stays separate from goal spending and the agent's evidence. */
import { useTranslation } from 'react-i18next';
import type { GoalReport } from '@/generated/api-types';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { RelativeTime } from '@/ui/RelativeTime';
import { UserChip } from '@/ui/UserChip';
import { ProducerFact, tokenWords, usageWords } from '@/features/summaries/facts';

export function GoalHelperFacts({ report, checking, stateSaid }: { report: GoalReport | null; checking: boolean; stateSaid: boolean }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? 'en';
  if (report?.status !== 'met') return null;
  const helper = report.helper;
  const checked = report.checked_by === 'helper' || helper?.checked === true;
  const unchecked = report.outcome === 'unchecked';
  if ((helper === undefined || helper === null) && !checked && !unchecked) return null;
  const fragment = checking || stateSaid ? undefined : checked && report.checked_by !== 'person' && report.outcome !== null ? t('summaryFacts.helperChecked') : unchecked ? t('summaryFacts.notChecked') : undefined;
  return <FactDisclosure fragment={fragment} groups={[{ key: 'helper', kind: 'recorded_outcome', facts: [
    { key: 'producer', label: t('summaryFacts.producer'), value: <ProducerFact view={helper?.producer} /> },
    { key: 'owner', label: t('summaryFacts.usageOwner'), value: helper === null || helper === undefined ? undefined : helper.usage_owner === null ? t('summaryFacts.notShared') : <UserChip user={helper.usage_owner} /> },
    { key: 'input', label: t('summaryFacts.inputAllowance'), value: tokenWords(helper?.input_allowance_tokens, t, locale) },
    { key: 'output', label: t('summaryFacts.outputAllowance'), value: tokenWords(helper?.output_allowance_tokens, t, locale) },
    { key: 'turn', label: t('summaryFacts.lastTurn'), value: helper?.last_turn === null || helper?.last_turn === undefined ? undefined : t(`summaryFacts.coverage.${helper.last_turn}`) },
    { key: 'usage', label: t('summaryFacts.recordedUsage'), value: usageWords(helper?.usage, t, locale) },
    { key: 'at', label: t('summaryFacts.captured'), value: helper === null || helper === undefined ? undefined : <RelativeTime at={helper.captured_at} /> },
  ] }]} />;
}
