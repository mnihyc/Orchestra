/** Folder discovery is visible and cancellable before its files enter the shared uploader. */
import { X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { useViewWork } from '@/features/workbench/viewOpening';
import { useAppearance } from '@/lib/appearance';
import { droppedFiles } from '@/lib/drop';
import { formatBytes, formatNumber } from '@/lib/format';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { toast } from '@/ui/Toast';

interface Reading { id: string; label: string; files: number; bytes: number; error?: unknown }

export function useFileDrops(source: string) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const reader = useStore(client.session, (state) => state.readerVersion);
  const scope = `${reader ?? 0}:${source}`;
  const [state, setState] = useState<{ scope: string; rows: Reading[] }>({ scope, rows: [] });
  const work = useRef(new Map<string, AbortController>());
  const change = (update: (rows: Reading[]) => Reading[]) => setState((before) => ({ scope, rows: update(before.scope === scope ? before.rows : []) }));
  const remove = (id: string) => { work.current.get(id)?.abort(); work.current.delete(id); change((rows) => rows.filter((row) => row.id !== id)); };
  const read = (data: DataTransfer, label: string, publish: (files: File[]) => void) => {
    const id = crypto.randomUUID(), abort = new AbortController(); work.current.set(id, abort);
    change((rows) => [...rows, { id, label, files: 0, bytes: 0 }]);
    void droppedFiles(data, abort.signal, (files, bytes) => change((rows) => rows.map((row) => row.id === id ? { ...row, files, bytes } : row))).then((files) => {
      abort.signal.throwIfAborted();
      if (files.length === 0) toast({ tone: 'info', title: t('uploads.noFiles') }); else publish(files);
      remove(id);
    }).catch((error: unknown) => {
      if (!abort.signal.aborted) change((rows) => rows.map((row) => row.id === id ? { ...row, error } : row));
    });
  };
  useEffect(() => { const held = work.current; return () => { for (const abort of held.values()) abort.abort(); held.clear(); }; }, [scope]);
  const rows = state.scope === scope ? state.rows : [];
  useViewWork(rows.length > 0);
  useEffect(() => {
    if (rows.length === 0) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault(); window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [rows.length]);
  return { read, blocked: rows.length > 0, progress: rows.length === 0 ? null : <div className="flex flex-col gap-1">{rows.map((row) => <div key={row.id}>
    <div role="status" className="flex min-w-0 items-center gap-2 text-sm text-text-muted tabular-nums"><span className="min-w-0 flex-1 truncate">{row.label} · {row.error === undefined ? t('uploads.reading', { count: row.files, number: formatNumber(row.files, locale), bytes: formatBytes(row.bytes, locale) }) : t('files.upload.readFailed')}</span><IconButton size="sm" label={t(row.error === undefined ? 'uploads.cancelRead' : 'common.dismiss')} icon={<X aria-hidden />} onClick={() => remove(row.id)} /></div>
    {row.error !== undefined && <ErrorAlert error={row.error} />}
  </div>)}</div> };
}
