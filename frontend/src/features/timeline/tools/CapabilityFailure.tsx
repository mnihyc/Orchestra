/** Reported capability failures keep their reason, allowance and reset next to the helper link. */
import { Trans, useTranslation } from 'react-i18next';
import { capabilityAnswer } from './CapabilityHelper';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';

const objectValue = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

export function CapabilityFailure({ result }: { result: unknown }) {
  const { t, i18n } = useTranslation();
  const answer = capabilityAnswer(result);
  if (!objectValue(answer) || answer.state !== 'failed' || answer.isError !== true || !objectValue(answer.failure)) return null;
  const failure = answer.failure;
  if (typeof failure.code !== 'string' || failure.code.length === 0 || typeof failure.message !== 'string' || failure.message.length === 0) return null;
  const language = i18n.resolvedLanguage ?? i18n.language;
  const translated = objectValue(failure.text) ? failure.text[language] ?? failure.text.en : null;
  const known = typeof failure.reason === 'string' && i18n.exists(`capabilityRoutes.reasons.${failure.reason}`) ? t(`capabilityRoutes.reasons.${failure.reason}`) : null;
  const message = typeof translated === 'string' ? translated : known ?? failure.message;
  const reset = typeof failure.resets_at === 'string' && Number.isFinite(Date.parse(failure.resets_at)) ? failure.resets_at : null;
  return <InlineAlert tone="warning" title={t('capabilityBackend.failed')}>
    <span className="inline-flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1" data-capability-failure>
      <span>{message}</span>
      {typeof failure.window === 'string' && failure.window.length > 0 && <span>{t('capabilityBackend.allowance', { window: failure.window })}</span>}
      {reset !== null && <span><Trans i18nKey="capabilityBackend.resets" components={{ time: <RelativeTime at={reset} /> }} /></span>}
    </span>
  </InlineAlert>;
}
