/** Release notes are a part of this application's bundle, with links into the same build's help. */
import { useTranslation } from 'react-i18next';
import { GuideLink } from '@/features/help/GuideLink';

export function WhatsNew() {
  const { t } = useTranslation();
  return <section className="flex flex-col gap-4 p-4" aria-label={t('helpRelease.news.title')}>
    <h2 className="text-lg font-semibold">{t('helpRelease.news.version')}</h2>
    <p className="text-sm text-text-muted">{t('helpRelease.news.about')}</p>
    <ul className="flex flex-col gap-4">
      <li><p className="text-sm">{t('usagePerf.releaseNote')}</p></li>
      <li><p className="text-sm">{t('helpRelease.news.approvals')}</p><GuideLink guide="how-approvals-work" named /></li>
      <li><p className="text-sm">{t('helpRelease.news.ports')}</p><GuideLink guide="let-agents-open-ports" named /></li>
      <li><p className="text-sm">{t('helpRelease.news.live')}</p><GuideLink guide="change-a-running-environment" named /></li>
      <li><p className="text-sm">{t('helpRelease.news.holders')}</p><GuideLink guide="who-can-see-and-do-what" named /></li>
    </ul>
  </section>;
}
