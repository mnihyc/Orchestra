/** Known native availability retires the input surface without destroying its saved draft or upload lifetime. */
import { useQuery } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { askModelsQuery } from '@/api/ask';
import { nodeQuery } from '@/api/nodes';
import { ApiError } from '@/api/client';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { AskModelsReply } from '@/generated/api-types';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { Button } from '@/ui/Button';
import { askModelExclusionText } from './modelChoices';

function ConfigureModels({ root }: { root: string }) {
  const { t } = useTranslation();
  const node = useQuery(nodeQuery(root));
  if (node.data?.kind !== 'org' || node.isError) {
    const privateRead = node.error instanceof ApiError && [401, 403, 404, 410].includes(node.error.status);
    return <span className="inline-flex min-w-0 flex-wrap items-center gap-1">
      <Button size="sm" variant="ghost" blocked reason={t(node.isPending ? 'common.loading' : 'implicitAvailability.detailsUnavailable')}>{t('ask.configureModels')}</Button>
      {node.isError && !privateRead && <Button size="sm" variant="ghost" pending={node.isFetching} onClick={() => void node.refetch()}>{t('common.tryAgain')}</Button>}
    </span>;
  }
  const org = node.data;
  return <SourceSettingLink source={{ scope: 'org', node: { id: org.id, kind: 'org', name: org.name, redacted: false } }}
    def={{ key: 'ask.models', class: 'policy' }} target={{ kind: 'node', id: root }} fallback={org.name} text={t('ask.configureModels')} />;
}

export function AskModelsBlocked({ root, reply, stop }: { root: string; reply: AskModelsReply; stop?: ReactNode }) {
  const { t } = useTranslation();
  const reason = reply.blocked_reason;
  const text = reason === 'not_configured' ? t('ask.blockedNotConfigured') : reason === 'empty' ? t('ask.blockedEmpty')
    : reason === 'excluded_by_policy' ? t('ask.blockedByPolicy') : t('ask.blockedNoUsableModels');
  const detail = reason === 'no_usable_models' && reply.models.length === 1 ? reply.models[0]?.unavailable_reason : null;
  return <div className="flex shrink-0 flex-col gap-2 border-t border-border px-3 py-3">
    <StateLine title={detail === null || detail === undefined ? text : `${text} · ${askModelExclusionText(detail, t)}`} action={reply.writable ? <ConfigureModels root={root} /> : undefined} />
    {stop}
  </div>;
}

export function AskModelAvailability({ root, children, stop }: { root: string; children: (available: boolean) => ReactNode; stop?: ReactNode }) {
  const shown = useViewShown();
  const { t } = useTranslation();
  const models = useQuery({ ...askModelsQuery(root), enabled: shown && root !== '' });
  const available = root !== '' && models.isSuccess && models.data.blocked_reason === null;
  const state = root === '' ? <StateLine title={t('ask.noOrg')} /> : models.isPending ? <SkeletonRows rows={2} />
    : models.isError ? <ErrorState compact error={models.error} onRetry={() => void models.refetch()} />
    : models.data.blocked_reason !== null ? <AskModelsBlocked root={root} reply={models.data} stop={stop} /> : null;
  return <>
    {state}
    {!available && (models.isError || models.isPending) && stop}
    <div hidden={!available} className="min-w-0 shrink-0">{children(available)}</div>
  </>;
}
