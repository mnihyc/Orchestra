/** Every personal page shares the scope frame and the sections in its settings sidebar.
 *
 * A person's own settings are a few pages and one place to go from any of them to any other. This draws
 * that navigation once, with the page in view marked, and gives the page its heading, so that each page is
 * only its content. Pages that belong to other parts of the work (notifications, Your exports) stand in the
 * same navigation and open their own screens.
 */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { SettingsFrame } from '@/features/settings/Frame';
import { PERSONAL_PAGES, personalSections, personalTitle, type PersonalPage } from '@/features/settings/navigation';
import { useScreenHeading } from '@/shell/screen';
import { ACTIONS } from '@/ui/actions';

export type { PersonalPage } from '@/features/settings/navigation';

export interface PersonalFrameProps {
  page: PersonalPage;
  /** One or two sentences under the name that say what the page is for. */
  lead: ReactNode;
  children: ReactNode;
}

export function PersonalFrame({ page, lead, children }: PersonalFrameProps) {
  const { t } = useTranslation();
  const current = PERSONAL_PAGES.find((entry) => entry.key === page);
  const Icon = current?.Icon ?? ACTIONS.profile.Icon;
  const title = personalTitle(page, t);
  useScreenHeading({ title, icon: Icon, trail: [{ label: t('screens.sections.settings'), to: '/settings/profile' }] });
  return (
    <SettingsFrame title={title} lead={lead} label={t('settings.personal.nav')} current={page} items={personalSections(t)}>{children}</SettingsFrame>
  );
}
