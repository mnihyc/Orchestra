/** Current public-document policy, its creation reach and the setting that controls it. */
import { useQuery } from '@tanstack/react-query';
import { Settings2 } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { settingsQuery } from '@/api/settings';
import { WhereSet } from '@/features/help/WhereSet';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function DocumentPolicy({ topicId }: { topicId: string }) {
  const { t } = useTranslation();
  const settings = useQuery({ ...settingsQuery(topicId), staleTime: 0 });
  if (settings.isPending) return <SkeletonRows rows={2} />;
  if (settings.isError) return <ErrorState compact error={settings.error} onRetry={() => void settings.refetch()} />;
  const policy = settings.data.find((setting) => setting.key === 'sharing.agent_documents');
  if (policy === undefined || !['none', 'read', 'write'].includes(String(policy.value))) return <p className="text-sm text-text-muted">{t('people.policyUnavailable')}</p>;
  return <section aria-label={t('people.documentsPolicy')} className="flex flex-col gap-2 rounded-lg border border-border bg-bg-subtle p-3 text-sm">
    <h3 className="font-semibold text-text">{t('people.documentsPolicy')}</h3>
    <p>{t(policy.value === 'write' ? 'people.policyWrite' : policy.value === 'read' ? 'people.policyRead' : 'people.policyNone')}</p>
    <p className="text-text-muted">{t('people.privateExcluded')}</p>
    <p className="text-xs text-text-muted">{t('people.conversationsSeparate')}</p>
    <WhereSet setting={policy} def={{ key: 'sharing.agent_documents', class: 'policy' }} target={{ kind: 'node', id: topicId }} />
    <a href={`/topic/${topicId}?tab=settings&section=policies&setting=sharing.agent_documents`} className="flex items-center gap-1 self-start text-accent-text hover:underline"><Settings2 aria-hidden className="size-4" />{t('people.policySetting')}</a>
  </section>;
}
