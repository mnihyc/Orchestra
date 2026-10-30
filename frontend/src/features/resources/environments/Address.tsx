/** Shows the server's short address unchanged, so it is the same locator the finder accepts. */
import { useTranslation } from 'react-i18next';

export function EnvironmentAddress({ address }: { address: string }) {
  const { t } = useTranslation();
  return <span className="text-sm wrap-anywhere text-text-muted" title={t('environmentAddress.hint', { address })}>{address}</span>;
}
