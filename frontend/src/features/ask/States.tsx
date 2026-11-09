/** Ask's states use the same named chips, notices and recovery controls as the other views. */
import { useTranslation } from 'react-i18next';
import type { AskConversation } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateChip } from '@/ui/StateChip';
import { StateLine } from '@/ui/StateLine';

export function AskStatus({ conversation, stopped }: { conversation: AskConversation; stopped: boolean }) {
  const { t } = useTranslation();
  const active = conversation.state === 'running';
  return <StateChip state={active ? 'running' : conversation.state === 'closed' ? 'blocked' : 'idle'} label={t(active ? 'ask.running' : conversation.state === 'closed' ? 'ask.closed' : conversation.state === 'kept' ? 'ask.kept' : stopped ? 'ask.stopped' : 'ask.ready')} />;
}
export function AskGovernance({ conversation }: { conversation: AskConversation }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  return <><span>{t('ask.retention', { days: formatNumber(conversation.retention_days, locale) })} · {t('ask.expires')} <RelativeTime at={conversation.expires_at} /></span>
    {conversation.admin_inspectable && <span>{t('ask.adminNotice')}</span>}{conversation.inspecting && <span>{t('ask.inspection')}</span>}</>;
}
export function AskEmptyState({ loading, error, deleted, retry, fresh }: { loading: boolean; error: unknown; deleted: boolean; retry: () => void; fresh: () => void }) {
  const { t } = useTranslation();
  return loading ? <SkeletonRows rows={4} /> : error !== null ? <ErrorState error={error} onRetry={retry} /> : deleted ? <StateLine title={t('ask.deleted')} action={<Button size="sm" onClick={fresh}>{t('ask.new')}</Button>} /> : <StateLine title={t('ask.empty')} />;
}

export function AskToolProgress({ name }: { name: string }) {
  const { t } = useTranslation();
  const label = name === 'search' ? t('ask.toolSearch') : name === 'list' ? t('ask.toolList') : name === 'history' ? t('ask.toolHistory') : name === 'docs' ? t('ask.toolDocs') : name === 'find_operation' ? t('ask.toolFind') : t('ask.toolRead');
  return <div className="py-1"><StateChip state="running" label={label} /></div>;
}
