/** A completed account creation offers sign-in without opening another session. */
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import { StateLine } from '@/ui/StateLine';
import { linkClasses } from '@/ui/link';

export function CreatedAccount() {
  const { t } = useTranslation();
  return <StateLine compact role="status" message={t('auth.creation.alreadyCreated')}
    action={<Link to="/login" className={linkClasses}>{t('auth.links.signIn')}</Link>} />;
}
