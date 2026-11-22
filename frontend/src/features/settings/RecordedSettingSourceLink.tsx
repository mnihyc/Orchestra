/** A recorded origin keeps its identity; its name is read only under current authority on intent. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { READ_NOW } from '@/api/freshness';
import { nodeQuery } from '@/api/nodes';
import type { SettingsTarget } from '@/api/settings';
import { SourceSettingLink } from '@/features/settings/SourceSettingLink';
import type { SettingSource } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';

/** Mount only in the subject's existing open fact body; it starts no mutation or model work. */
export function RecordedSettingSourceLink({ source, setting, target }: { source: SettingSource; setting: string; target: SettingsTarget }) {
  const { t } = useTranslation();
  const found = useQuery({ ...nodeQuery(source.node?.id ?? ''), ...READ_NOW, enabled: source.node !== null, retry: false, refetchOnMount: 'always' });
  if (source.node === null) return <SourceSettingLink source={source} def={{ key: setting, class: 'policy' }} target={target} fallback={t('factDisclosure.notReported')} />;
  if (found.isPending || found.isFetching) return <span>{t('common.loading')}</span>;
  if (found.isError) {
    if (found.error instanceof ApiError && [403, 404].includes(found.error.problem.status)) return <span>{t('settings.compact.hiddenSource')}</span>;
    return <ErrorAlert error={found.error} action={<Button size="sm" onClick={() => { void found.refetch(); }}>{t('common.tryAgain')}</Button>} />;
  }
  const resolved: SettingSource = { ...source, node: { id: source.node.id, kind: source.node.kind, name: found.data.name, redacted: false } };
  return <SourceSettingLink source={resolved} def={{ key: setting, class: 'policy' }} target={target} fallback={t('factDisclosure.notReported')} />;
}
