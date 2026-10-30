/** A failed panel keeps its neighbours, their drafts and its own Retry in place. */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { RefreshCw } from 'lucide-react';

import { Button } from '@/ui/Button';
import { ErrorBoundary } from '@/ui/ErrorBoundary';
import { StateLine } from '@/ui/StateLine';

export function ContentFailure({ retry, className }: { retry: () => void; className?: string | undefined }) {
  const { t } = useTranslation();
  return <StateLine role="alert" tone="danger" compact className={className} message={t('ui.error.partFailed')}
    action={<Button size="sm" icon={<RefreshCw aria-hidden />} onClick={retry}>{t('common.tryAgain')}</Button>} />;
}

export function ContentBoundary({ children, resetKey, className }: { children: ReactNode; resetKey?: unknown; className?: string | undefined }) {
  return <ErrorBoundary resetKey={resetKey} fallback={(_error, retry) => <ContentFailure retry={retry} className={className} />}>{children}</ErrorBoundary>;
}
