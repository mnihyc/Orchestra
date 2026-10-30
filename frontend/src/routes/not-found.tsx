/** Unknown route screen inside the shell.
 *
 * What a path that no route answers shows: that there is nothing here, and the way back to where
 * there is. It stands in whatever frame the visitor is in, so somebody signed in keeps their
 * workbench round it.
 */
import { Link } from '@tanstack/react-router';
import { Compass } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useScreenHeading } from '@/shell/screen';
import { ActionIcon } from '@/ui/actions';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';

const HomeIcon = () => <ActionIcon action="home" />;

export function NotFound() {
  const { t } = useTranslation();
  useScreenHeading({ title: t('craft.pageNotFound'), icon: Compass, trail: [{ label: t('craft.home'), to: '/' }] });
  return (
    <EmptyState
      className="h-full"
      icon={<Compass aria-hidden />}
      title={t('shell.notFound.title')}
      titleAs="h1"
      description={t('shell.notFound.lead')}
      action={
        <Button variant="primary" asChild>
          <Link to="/"><HomeIcon />{t('actions.home.label')}</Link>
        </Button>
      }
    />
  );
}
