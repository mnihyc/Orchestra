/** Executor choices use machine declarations and keep one visible page mounted. */
import { useTranslation } from 'react-i18next';
import type { ExecutorOffer, MachineExecutor } from '@/generated/api-types';
import { localizedText } from '@/features/resources/executorNames';
import { CurrentDetailPage } from '@/features/resources/CurrentDetailPage';
import { CheckboxList, type CheckboxOption } from '@/ui/Checkbox';
import { StateLine } from '@/ui/StateLine';

/** Index one reported page once; changing a selection does not rebuild its declarations. */
export function executorOfferIndex(executors: readonly MachineExecutor[]) {
  const offers: ExecutorOffer[] = [];
  const byValue = new Map<string, ExecutorOffer>();
  const aliases = new Map<string, string>();
  for (const { executor } of executors) for (const offer of executor.offers) {
    if (byValue.has(offer.value)) continue;
    offers.push(offer); byValue.set(offer.value, offer);
  }
  for (const offer of offers) if (offer.alias !== null && !byValue.has(offer.alias) && !aliases.has(offer.alias)) aliases.set(offer.alias, offer.value);
  return { offers, byValue, aliases };
}

type OfferIndex = ReturnType<typeof executorOfferIndex>;

/** An older bare offer stays checked under its declared canonical value. */
export function canonicalExecutorOffers(values: readonly string[], index: OfferIndex): string[] {
  return [...new Set(values.map((value) => index.aliases.get(value) ?? value))];
}

/** Missing saved offers may be retained or cleared, but are never proposed as new choices. */
export function ExecutorOffers({ index, selected, saved, onValueChange, disabled }: {
  index: OfferIndex; selected: readonly string[]; saved: readonly string[]; onValueChange: (values: string[]) => void; disabled: boolean;
}) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const missing = [...new Set([...saved, ...selected])].filter((value) => !index.byValue.has(value));
  const count = index.offers.length + missing.length;
  if (count === 0) return <StateLine compact message={t('machines.policy.noExecutors')} />;
  return <CurrentDetailPage count={count} blocked={disabled}>{(from, to) => {
    const options: CheckboxOption<string>[] = [];
    for (let row = from; row < to; row += 1) {
      const offer = index.offers[row];
      if (offer !== undefined) options.push({ value: offer.value, name: localizedText(offer.title, locale) ?? t('ui.nameUnknown'), description: localizedText(offer.summary, locale) ?? t('machines.policy.executorSummaryUnknown') });
      else {
        const value = missing[row - index.offers.length];
        if (value !== undefined) options.push({ value, name: t('machines.policy.savedOffer', { value }), description: t('machines.policy.unreportedOffer'), disabled: !selected.includes(value) });
      }
    }
    return <CheckboxList label={t('machines.policy.value')} value={selected} onValueChange={onValueChange} options={options} disabled={disabled} />;
  }}</CurrentDetailPage>;
}
