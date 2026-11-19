/** The handoff's recorded input stays separate from carried history and the complete transcript. */
import type { TFunction } from 'i18next';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { HandoffResult, SummaryTurnRange } from '@/generated/api-types';
import { formatBytes, formatNumber } from '@/lib/format';
import type { FactGroup } from '@/ui/FactDisclosure';
import { Input } from '@/ui/Input';
import { RelativeTime } from '@/ui/RelativeTime';

function rangeWords(range: SummaryTurnRange, locale: string): string {
  return range.first === range.through ? formatNumber(range.first, locale) : `${formatNumber(range.first, locale)}–${formatNumber(range.through, locale)}`;
}

function Ranges({ ranges, locale, label }: { ranges: readonly SummaryTurnRange[]; locale: string; label: string }) {
  const { t } = useTranslation();
  const [query, search] = useState('');
  if (ranges.length === 0) return <>{t('placeFacts.noTurns')}</>;
  const many = ranges.length > 10;
  const words = ranges.map((range) => rangeWords(range, locale)).filter((word) => !many || word.includes(query.trim()));
  return <div className="flex min-w-0 max-w-full flex-col gap-1" data-handoff-ranges>
    {many && <Input label={label} hideLabel type="search" value={query} onChange={(event) => search(event.target.value)} className="h-(--control-sm) text-sm" />}
    <div className="max-h-48 overflow-y-auto" data-handoff-range-list>
      {words.length === 0 ? t('placeFacts.noMatchingTurns') : words.map((word) => <span key={word} className="block tabular-nums">{word}</span>)}
    </div>
  </div>;
}

export function handoffFragment(result: HandoffResult | null | undefined, t: TFunction, locale: string): string | undefined {
  const input = result?.input_coverage;
  if (input === null || input === undefined) return undefined;
  const omitted = input.oversized_turn_ranges.reduce((sum, range) => sum + range.through - range.first + 1, 0);
  return omitted > 0 ? t('placeFacts.turnsOmitted', { count: omitted, n: formatNumber(omitted, locale) }) : undefined;
}

export function handoffInputGroup(result: HandoffResult | null | undefined, t: TFunction, locale: string): FactGroup {
  const input = result?.input_coverage;
  const ranges = (value: readonly SummaryTurnRange[] | undefined, label: string) => value === undefined ? null
    : <Ranges key={`${input?.captured_at}:${label}`} ranges={value} locale={locale} label={label} />;
  return { key: 'handoff-input', title: t('placeFacts.handoffInput'), facts: [
    { key: 'read', label: t('placeFacts.turnReadAllowance'), value: input === null || input === undefined ? null : formatBytes(input.read_turn_bytes, locale) },
    { key: 'included', label: t('placeFacts.inputTurns'), value: ranges(input?.included_turn_ranges, t('placeFacts.inputTurns')) },
    { key: 'omitted', label: t('placeFacts.omittedTurns'), value: ranges(input?.oversized_turn_ranges, t('placeFacts.omittedTurns')) },
    ...(input?.omitted_prefix === true ? [{ key: 'prefix', label: t('placeFacts.omittedPrefix'), value: t('placeFacts.earlierInputOmitted') }] : []),
    { key: 'at', label: t('placeFacts.recorded'), value: input === null || input === undefined ? null : <RelativeTime at={input.captured_at} /> },
  ] };
}
