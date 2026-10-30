/** The connection failure names its cause once, followed by a concrete next step. */
import { useTranslation } from 'react-i18next';
import { AuthCard } from '@/features/auth/Frame';
import { ErrorState } from '@/ui/ErrorState';
import { useErrorText } from '@/ui/errorText';

export function ConnectionFailure({ error, onRetry }: { error: unknown; onRetry: () => void }) {
  const { t } = useTranslation();
  const text = useErrorText()(error);
  return <AuthCard title={t('shell.unreachable')}>
    <ErrorState compact error={error} title={text.message} message={t('shell.checkConnection')} onRetry={onRetry} />
  </AuthCard>;
}
