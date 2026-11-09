/** One document's override, through the same schema control as its place defaults. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { documentSummaryOptionsKey, documentSummaryOptionsQuery, setDocumentSummaryOptions, summaryReaderKey } from '@/api/summaries';
import { SettingChoiceContext } from '@/features/settings/choiceCopy';
import { shapeOf } from '@/features/settings/settingsModel';
import { settingFieldName } from '@/features/settings/settingText';
import { ValueControl } from '@/features/settings/ValueControl';
import type { SummaryTargetOptions } from '@/generated/api-types';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

const family = SETTINGS_REGISTRY.find((entry) => entry.key === 'summary.targets');
const familyShape = family === undefined ? null : shapeOf(family.schema);
const shape = familyShape?.type === 'object' ? familyShape.members.find((member) => member.name === 'document')?.shape ?? null : null;

export function DocumentSummaryOptionsDialog({ documentId, onOpenChange }: { documentId: string; onOpenChange: (open: boolean) => void }) {
  const reader = useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion ?? 0, state.status]));
  return <Options key={JSON.stringify([reader, documentId])} documentId={documentId} reader={reader} onOpenChange={onOpenChange} />;
}

function Options({ documentId, reader, onOpenChange }: { documentId: string; reader: string; onOpenChange: (open: boolean) => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const read = useQuery(documentSummaryOptionsQuery(documentId));
  const [draft, setDraft] = useState<SummaryTargetOptions | undefined>(undefined);
  const alive = useRef(true);
  const formId = useId();
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);
  const write = useMutation({
    mutationKey: [...documentSummaryOptionsKey(documentId), 'write'],
    retry: false,
    mutationFn: (options: SummaryTargetOptions | null) => {
      if (read.data === undefined || !read.data.writable || read.data.locked || reader !== summaryReaderKey()) throw new DOMException('The summary opening changed', 'AbortError');
      return setDocumentSummaryOptions(documentId, options, read.data.revision);
    },
    onSuccess: async () => {
      if (reader !== summaryReaderKey()) return;
      await cache.invalidateQueries({ queryKey: documentSummaryOptionsKey(documentId) });
      if (alive.current && reader === summaryReaderKey()) onOpenChange(false);
    },
  });
  const value = draft ?? read.data?.options;
  const writable = read.data?.writable === true && read.data.locked === false;
  const reason = read.data?.locked === true ? t('summaryOptions.document.locked')
    : !writable ? t('summaryOptions.document.readOnly')
      : draft !== undefined || read.data?.own != null ? t('summaryOptions.document.own') : t('summaryOptions.document.inherited');
  const reload = () => { setDraft(undefined); write.reset(); void read.refetch(); };
  const next = <Button size="sm" onClick={reload}>{t('summaryOptions.document.reload')}</Button>;
  return <Dialog open onOpenChange={onOpenChange} title={t('summaryOptions.document.title')} size="md"
    footer={read.data === undefined || read.isError || shape === null ? undefined : <div className="flex w-full min-w-0 flex-wrap items-center gap-2">
      <div className="min-w-0 flex-1 text-sm text-text-muted"><span>{reason}</span><span className="block">{t('summaryOptions.document.interval', { seconds: read.data.min_interval_secs })}</span></div>
      {read.data.own != null && !read.data.locked && <Button variant="ghost" disabled={!writable || write.isPending} onClick={() => write.mutate(null)}>{t('summaryOptions.document.useDefault')}</Button>}
      <Button variant="secondary" onClick={() => onOpenChange(false)}>{t('common.cancel')}</Button>
      <Button variant="primary" type="submit" form={formId} pending={write.isPending} disabled={!writable || draft === undefined || write.isPending}>{t('common.save')}</Button>
    </div>}>
    {read.isPending ? <SkeletonRows read={read} rows={6} />
      : read.isError ? <ErrorAlert error={read.error} action={<Button size="sm" onClick={() => void read.refetch()}>{t('common.tryAgain')}</Button>} />
        : shape === null ? <ErrorAlert error={new Error(t('summaryOptions.document.unavailable'))} />
          : <form id={formId} className="flex flex-col gap-3" onSubmit={(event) => {
            event.preventDefault();
            if (event.currentTarget.reportValidity() && writable && draft !== undefined && !write.isPending) write.mutate(draft);
          }}>
            <SettingChoiceContext.Provider value="summary.targets">
              <ValueControl shape={shape} value={value} label={t('summaryOptions.document.title')} hideLabel
                memberLabel={(name) => settingFieldName(name, 'summary.targets', t)} disabled={!writable || write.isPending}
                onChange={(next) => setDraft(next as SummaryTargetOptions)} />
            </SettingChoiceContext.Provider>
            {write.isError && <ErrorAlert error={write.error} action={next} />}
          </form>}
  </Dialog>;
}
