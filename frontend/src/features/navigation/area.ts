/** The addressed area names its navigation; choosing an org never changes these sections. */
import { useRouterState } from '@tanstack/react-router';
import { CircleQuestionMark, Settings, ShieldUser } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { settingsNavigation } from '@/features/settings/navigation';

export type NavigationArea = 'personal' | 'system' | 'help' | 'place';

export function navigationArea(pathname: string, search: Record<string, unknown> = {}): NavigationArea | null {
  const settings = settingsNavigation(pathname, search);
  if (settings !== null) return settings.kind === 'personal' || settings.kind === 'system' ? settings.kind : 'place';
  const root = pathname.split('/')[1];
  return root === 'settings' ? 'personal' : root === 'admin' ? 'system' : root === 'help' ? root : null;
}

const ICONS = { personal: Settings, system: ShieldUser, help: CircleQuestionMark, place: Settings };

/** This is route metadata only: no org, guide, report or resource reader is started. */
export function useAreaNavigation() {
  const location = useRouterState({ select: (state) => state.location });
  const { t } = useTranslation();
  const area = navigationArea(location.pathname, location.search);
  return area === null ? null : { kind: area, name: t(`workbench.areaNavigation.${area}.name`),
    description: t(`workbench.areaNavigation.${area}.description`), Icon: ICONS[area] };
}
