/** Ordinary readable documents offered by the place's current template setting. */
import { useQueries, useQuery } from '@tanstack/react-query';
import { FileText } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { documentQuery } from '@/api/documents';
import { ApiError } from '@/api/client';
import { settingsQuery } from '@/api/settings';
import { CopyDocumentDialog } from '@/features/people/CopyDocument';
import type { Document } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';

export function Templates({ nodeId }: { nodeId: string }) {
  const { t } = useTranslation();
  const settings = useQuery(settingsQuery(nodeId));
  const setting = settings.isError ? undefined : settings.data?.find((row) => row.key === 'docs.templates')?.value;
  const ids = Array.isArray(setting) ? [...new Set(setting.filter((id): id is string => typeof id === 'string'))] : [];
  const reads = useQueries({ queries: ids.map((id) => ({ ...documentQuery(id), retry: false })) });
  const [chosen, setChosen] = useState<Document | null>(null);
  const visible = reads.filter((read) => read.isSuccess && !read.data.deleted);
  const errors = [settings, ...reads].filter((read) => read.isError);
  const privateRead = errors.some((read) => read.error instanceof ApiError && [401, 403, 404, 410].includes(read.error.status));
  const retryable = errors.filter((read) => !(read.error instanceof ApiError && [401, 403, 404, 410].includes(read.error.status)));
  const failure = retryable[0]?.error;
  const pending = settings.isPending || reads.some((read) => read.isPending);
  const deleted = reads.some((read) => read.isSuccess && read.data.deleted);
  if (!pending && !settings.isError && ids.length === 0) return null;
  return <div role="group" aria-label={t('people.templates')} className="flex min-w-0 flex-wrap items-center gap-2">
    <span className="text-sm text-text-muted">{t('people.templates')}</span>
    {pending && <SkeletonRows rows={1} />}
    {visible.map((read) => read.data === undefined ? null : <Button key={read.data.id} size="sm" icon={<FileText aria-hidden />} onClick={() => setChosen(read.data)}>{read.data.title}</Button>)}
    {(privateRead || deleted) && <StateLine compact message={t('implicitAvailability.templatesUnavailable')} />}
    {failure !== undefined && <ErrorState compact error={failure} message={t('implicitAvailability.templatesFailed')} onRetry={() => { for (const read of retryable) void read.refetch(); }} />}
    {chosen !== null && <CopyDocumentDialog source={chosen} destination={nodeId} onClose={() => setChosen(null)} />}
  </div>;
}
