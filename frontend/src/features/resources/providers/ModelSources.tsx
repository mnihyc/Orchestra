/** The same bounded source card on saved models and suggestions, without accepting or changing either. */
import { useTranslation } from 'react-i18next';
import type { ModelSuggestion, ProviderModel } from '@/generated/api-types';
import { formatContext } from '@/features/models/context';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { Popover } from '@/ui/Popover';
import { RelativeTime } from '@/ui/RelativeTime';
import { modelObservations, type ModelField, type ModelObservation } from './modelOrigins';

export function ModelSources({ observed, current, at }: { observed: ModelSuggestion | undefined; current?: ProviderModel; at: string | null }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  if (observed === undefined) return null;
  const fields = modelObservations(observed, current);
  if (fields.length === 0) return <span className="text-sm text-text-muted">{t('providerFill.unconfirmed')}</span>;
  const label = (field: ModelField): string => t(`providerFill.fields.${field}`);
  const changes = fields.filter((field) => field.origin === 'vendor' && field.differs);
  const value = (field: ModelObservation): string => {
    if (typeof field.value === 'boolean') return t(field.value ? 'providerFill.yes' : 'providerFill.no');
    if (typeof field.value === 'number') return field.field === 'window' || field.field === 'context' || field.field === 'output' ? formatContext(field.value, locale) : formatNumber(field.value, locale);
    if (Array.isArray(field.value)) return field.value.length === 0 ? t('providerFill.none') : field.value.map(String).join(', ');
    return typeof field.value === 'string' ? field.value : '–';
  };
  return <div className="flex min-w-0 flex-col items-start gap-1" data-model-observation={observed.model_name}>
    <Popover label={t('providerFill.reportedValues')} className="w-80" trigger={<Button size="sm" variant="ghost">{t('providerFill.reportedValues')}</Button>}>
      <div className="flex min-w-0 flex-col gap-3">
        <span className="text-sm text-text-muted">{at === null ? t('providerFill.timeUnknown') : <RelativeTime at={at} />}</span>
        <dl className="flex min-w-0 flex-col gap-3 text-sm">
          {fields.map((field) => <div key={field.pointer} className="flex min-w-0 flex-col gap-1" data-observed-field={field.field}>
            <dt className="font-medium text-text">{label(field.field)}</dt>
            <dd className="flex min-w-0 flex-wrap items-baseline justify-between gap-2 wrap-anywhere"><span>{value(field)}</span><span className="text-text-muted">{t(field.origin === 'vendor' ? 'providerFill.sources.vendor' : 'providerFill.sources.catalogue')}</span></dd>
          </div>)}
        </dl>
      </div>
    </Popover>
    {changes.length > 0 && <span className="text-sm wrap-anywhere text-text-muted">{t('providerFill.vendorDiffers', { fields: changes.map((field) => label(field.field)).join(', ') })}</span>}
  </div>;
}
