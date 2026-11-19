/** Public page identity comes from existing navigation metadata, never an inactive body read. */
import type { TFunction } from 'i18next';
import { Boxes, CircleQuestionMark, HardDrive, Inbox, ListTree, Search, Share2, ShieldCheck } from 'lucide-react';
import { KINDS } from '@/features/resources/ResourceNav';
import { PERSONAL_PAGES, personalTitle } from '@/features/settings/navigation';
import { LISTED } from '@/features/admin/sections';
import type { PageTarget } from './pageTargets';
import type { TabName } from './store';

/** Only public page labels. Resource names and permission-dependent headings keep their native supplier. */
export function staticPageName(target: PageTarget, t: TFunction): Pick<TabName, 'title' | 'icon'> | null {
  const path = target.pathname.replace(/\/$/, '');
  const resource = KINDS.find((entry) => entry.to === path);
  if (resource !== undefined) return { title: t(`screens.${resource.name}.title`), icon: resource.icon };
  const personal = PERSONAL_PAGES.find((entry) => entry.to === path);
  if (personal !== undefined) return { title: personal.key === 'settingsUsage' ? t('usage.screenTitle') : personalTitle(personal.key, t), icon: personal.Icon };
  if (target.route === '/admin/$section/') {
    const section = LISTED.find((entry) => entry.id === target.params.section);
    return section === undefined ? null : { title: t(`admin.sections.${section.id}.title`), icon: section.icon };
  }
  switch (target.route) {
    case '/resources/': return { title: t('screens.sections.resources'), icon: Boxes };
    case '/everything/': return { title: t('everything.title'), icon: ListTree };
    case '/search/': return { title: t('screens.search.title'), icon: Search };
    case '/inbox/': return { title: t('screens.inbox.title'), icon: Inbox };
    case '/approvals/': return { title: t('screens.approvals.title'), icon: ShieldCheck };
    case '/shared/': return { title: t('screens.shared.title'), icon: Share2 };
    case '/storage/': return { title: t('storage.title'), icon: HardDrive };
    case '/help/': return { title: t('screens.help.title'), icon: CircleQuestionMark };
    default: return null;
  }
}
