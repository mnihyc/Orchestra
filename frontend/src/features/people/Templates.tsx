/** Ordinary readable documents offered by the place's current template setting. */
import { useQueries, useQuery } from '@tanstack/react-query';
import { FileText } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { documentQuery } from '@/api/documents';
import { settingsQuery } from '@/api/settings';
import { CopyDocumentDialog } from '@/features/people/CopyDocument';
import type { Document } from '@/generated/api-types';
import { Button } from '@/ui/Button';

export function Templates({ nodeId }: { nodeId: string }) {
  const { t } = useTranslation();
  const setting = useQuery(settingsQuery(nodeId)).data?.find((row) => row.key === 'docs.templates')?.value;
  const ids = Array.isArray(setting) ? [...new Set(setting.filter((id): id is string => typeof id === 'string'))] : [];
  const reads = useQueries({ queries: ids.map((id) => ({ ...documentQuery(id), retry: false, staleTime: 0 })) });
  const [chosen, setChosen] = useState<Document | null>(null);
  const visible = reads.filter((read) => read.isSuccess && !read.data.deleted);
  if (visible.length === 0) return null;
  return <div className="flex flex-wrap items-center gap-2"><span className="text-sm text-text-muted">{t('people.templates')}</span>{visible.map((read) => read.data === undefined ? null : <Button key={read.data.id} size="sm" icon={<FileText aria-hidden />} onClick={() => setChosen(read.data)}>{read.data.title}</Button>)}{chosen !== null && <CopyDocumentDialog source={chosen} destination={nodeId} onClose={() => setChosen(null)} />}</div>;
}
