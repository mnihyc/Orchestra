/** The thread's current personal-read choice, without an audience-derived override. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { settingsQuery } from '@/api/settings';
import { ArticleGuideLink } from '@/features/help/ArticleGuideLink';
import { Button } from '@/ui/Button';

export function PersonalReads({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const settings = useQuery({ ...settingsQuery(threadId), retry: false });
  const option = settings.data?.find((row) => row.key === 'orch_tools.owner_reads')?.value;
  const state = settings.isError ? 'unavailable' : settings.data === undefined ? 'loading'
    : option === true ? 'on' : option === false ? 'off' : 'unavailable';
  return <section aria-label={t('thread.audience.reads.title')} className="flex min-w-0 flex-wrap items-center gap-x-1 border-b border-border px-3 py-2 text-xs text-text-muted" data-testid="personal-read-state">
    <span className="font-medium text-text">{t('thread.audience.reads.title')}:</span>
    <span>{t(`thread.audience.reads.${state}`)}</span>
    <span aria-hidden>·</span><span>{t('thread.audience.reads.shared')}</span>
    {settings.isError && <Button size="sm" variant="ghost" onClick={() => void settings.refetch()}>{t('common.tryAgain')}</Button>}
    <ArticleGuideLink article="personal-read-tools" title={t('thread.audience.reads.guide')} />
  </section>;
}
