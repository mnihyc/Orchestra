/** The current native policy is separate from a private inventory or a historical turn's skills. */
import type { UseQueryResult } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import type { ThreadKnowledgeView } from '@/api/share';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import type { SettingSource } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { StateLine } from '@/ui/StateLine';

const definition = { key: 'agents.personal_skills', class: 'policy' as const };
type Read = Pick<UseQueryResult<ThreadKnowledgeView>, 'data' | 'error' | 'isPending' | 'isRefetching' | 'refetch'>;

function sourceKey(source: SettingSource): string {
  const node = source.node;
  return node === null ? source.scope === 'system' || source.scope === 'default' ? source.scope : 'private'
    : node.redacted || node.name === null ? 'private' : `${source.scope}:${node.id}`;
}

export function PersonalSkillsPolicyLine({ read, threadId }: { read: Read; threadId: string }) {
  const { t } = useTranslation();
  const words = 'threadViews.knowledge.personalSkills' as const;
  if (read.isPending) return <StateLine compact role="status" message={t(`${words}.checking`)} />;
  if (read.error !== null) {
    const status = read.error instanceof ApiError ? read.error.status : null;
    const key = status === 401 || status === 403 ? 'denied' : status === 404 || status === 410 ? 'private' : status === 501 ? 'unsupported' : 'failed';
    // The knowledge read's existing ErrorState owns its one retry action.
    return <StateLine compact role="status" message={t(`${words}.${key}`)} />;
  }
  const policy = read.data?.personal_skills;
  const retry = <Button variant="ghost" size="sm" pending={read.isRefetching} onClick={() => { void read.refetch(); }}>{t(`${words}.retry`)}</Button>;
  if (policy?.state === 'unavailable') return <StateLine compact role="status" message={t(`${words}.failed`)} action={retry} />;
  if (policy?.state === 'unsupported') return <StateLine compact role="status" message={t(`${words}.unsupported`)} />;
  if (policy?.state === 'allowed') return <StateLine compact role="status" message={t(`${words}.allowed`)} />;
  if (policy?.state !== 'off' || policy.reason !== 'policy_off') return <StateLine compact role="status" message={t(`${words}.notReported`)} />;
  const sources = [...new Map(policy.sources.map((source) => [sourceKey(source), source])).values()];
  return <StateLine compact role="status" title={t(`${words}.off`)}
    message={sources.length === 0 ? t(`${words}.sourceNotReported`) : <>{sources.map((source, index) => <span key={sourceKey(source)}>
      {index === 0 ? '· ' : ', '}
      {source.node === null && source.scope !== 'system' && source.scope !== 'default'
        ? t('settings.compact.hiddenSource')
        : <SourceSettingLink source={source} def={definition} target={{ kind: 'node', id: threadId }} inherited
          fallback={t('settings.compact.hiddenSource')} />}
    </span>)}</>}
    action={sources.length === 0 ? retry : undefined} />;
}
