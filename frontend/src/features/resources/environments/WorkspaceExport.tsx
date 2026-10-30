/** Workspace exports use the leased snapshot job's live result, then a single authenticated download. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { downloadEnvironmentExport, exportEnvironmentFiles } from '@/api/environments';
import { jobQuery } from '@/api/forks';
import { JobProgress } from '@/features/change/Progress';
import { saveFile } from '@/features/export/save';
import { record } from '@/features/resources/UnitCommands';
import type { Job } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { useErrorText } from '@/ui/errorText';

/** An Inbox result remains downloadable after the source environment is gone. */
export function SnapshotDownloadButton({ id, name }: { id: string; name: string }) {
  const { t } = useTranslation();
  const download = useMutation({ mutationFn: () => downloadEnvironmentExport(id), onSuccess: (blob) => saveFile(blob, `${name}.zip`) });
  return <span className="inline-flex flex-col gap-1">
    <Button size="sm" pending={download.isPending} onClick={() => download.mutate()}>{t('temporary.download')}</Button>
    {download.error !== null && <ErrorAlert error={download.error} />}
  </span>;
}

function ExportProgress({ initial, name, retry }: { initial: Job; name: string; retry: () => void }) {
  const { t } = useTranslation();
  const failureText = useErrorText();
  const job = useQuery({ ...jobQuery(initial.id), initialData: initial });
  const row = job.data;
  const result = record(row.result);
  const snapshot = typeof result.snapshot_id === 'string' ? result.snapshot_id : null;
  const finished = row.state === 'succeeded' && snapshot !== null;
  const failed = row.state === 'failed' || row.state === 'cancelled' || row.state === 'succeeded' && snapshot === null;
  return <div className="flex flex-col items-start gap-3" aria-live="polite">
    {job.error !== null && <ErrorAlert error={job.error} />}
    {finished ? <>
      {typeof result.missing_count === 'number' && result.missing_count > 0 && <p className="text-sm text-warning-text">{t('temporary.exportPartial')}</p>}
      <SnapshotDownloadButton id={snapshot} name={name} />
    </> : failed ? <>
      <p className="text-sm text-danger-text">{row.error === null ? t('temporary.exportFailed') : failureText(row.error).message}</p>
      <Button size="sm" onClick={retry}>{t('temporary.retry')}</Button>
    </> : <JobProgress job={row} heading={t('temporary.exporting')} reconnecting={job.failureCount > 0} />}
  </div>;
}

/** Starting an export takes one mutation; hidden or closed windows subscribe to no job. */
export function EnvironmentExportButton({ id, name }: { id: string; name: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const key = useActKey();
  const attempt = useRef(0);
  const exportFiles = useMutation({ mutationFn: () => exportEnvironmentFiles(id, key({ id, attempt: attempt.current })) });
  const retry = () => { attempt.current += 1; exportFiles.reset(); exportFiles.mutate(); };
  const start = () => { setOpen(true); if (exportFiles.isSuccess) retry(); else exportFiles.mutate(); };
  return <>
    <Button size="sm" pending={exportFiles.isPending} onClick={start}>{t('temporary.export')}</Button>
    {open && <Dialog open onOpenChange={setOpen} title={t('temporary.exportTitle')} footer={<Button onClick={() => setOpen(false)}>{t('common.close')}</Button>}>
      {exportFiles.error !== null ? <div className="flex flex-col items-start gap-3"><ErrorAlert error={exportFiles.error} /><Button size="sm" onClick={() => exportFiles.mutate()}>{t('temporary.retry')}</Button></div>
        : exportFiles.data !== undefined ? <ExportProgress initial={exportFiles.data} name={name} retry={retry} />
        : <p role="status" className="text-sm text-text-muted">{t('temporary.exporting')}</p>}
    </Dialog>}
  </>;
}
