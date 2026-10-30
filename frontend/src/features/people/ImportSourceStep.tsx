/** Files, folders and archives enter the import through one accessible source selection. */
import { Archive, FileUp, FolderOpen } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { DropTarget } from '@/features/composer/DropTarget';
import { useFileDrops } from '@/features/composer/FileDrops';
import { ImportPagination } from '@/features/people/ImportPagination';
import { importSource, readImportSource, SOURCE_BATCH, type ImportSource, type ImportSourceFault } from '@/features/people/importSource';
import { useAppearance } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';
import { ByteSize } from '@/ui/ByteSize';
import { ErrorAlert } from '@/ui/ErrorState';
import { StateLine } from '@/ui/StateLine';

/** A page of selected names keeps a large folder useful without hiding the rest. */
const SOURCE_PAGE = 50;

const FAULTS: Record<ImportSourceFault, 'invalidPath' | 'duplicatePath' | 'archiveAlone' | 'empty'> = {
  'invalid-path': 'invalidPath', 'duplicate-path': 'duplicatePath', 'several-archives': 'archiveAlone', empty: 'empty',
};

export interface ImportSourceStepProps {
  value: ImportSource | null;
  onChange: (source: ImportSource) => void;
  onReadingChange?: (reading: boolean) => void;
  disabled?: boolean;
}

/** Lets a person pick or drop a source without deciding a package from its filename. */
export function ImportSourceStep({ value, onChange, onReadingChange, disabled = false }: ImportSourceStepProps) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const files = useRef<HTMLInputElement>(null);
  const folder = useRef<HTMLInputElement>(null);
  const archive = useRef<HTMLInputElement>(null);
  const drops = useFileDrops('import-source');
  const [start, setStart] = useState(0);
  const [reading, setReading] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const inspection = useRef<AbortController | null>(null);
  const blocked = disabled || drops.blocked || reading;
  useEffect(() => { onReadingChange?.(drops.blocked || reading); }, [drops.blocked, reading, onReadingChange]);
  useEffect(() => () => inspection.current?.abort(), []);

  const choose = (picked: File[], oneArchive: boolean) => {
    if (picked.length === 0) return;
    setStart(0); setError(null);
    inspection.current?.abort();
    if (picked.length <= SOURCE_BATCH) { onChange(importSource(picked, oneArchive)); return; }
    const controller = new AbortController();
    inspection.current = controller;
    setReading(true);
    void readImportSource(picked, oneArchive, controller.signal).then((source) => {
      if (!controller.signal.aborted) onChange(source);
    }).catch((failure: unknown) => {
      if (!controller.signal.aborted) { setError(failure); onChange({ files: [], bytes: 0, fault: 'empty' }); }
    }).finally(() => { if (!controller.signal.aborted) setReading(false); });
  };
  const fromInput = (input: HTMLInputElement, oneArchive: boolean) => {
    choose(Array.from(input.files ?? []), oneArchive);
    // The same file can be chosen again after an error or a change of destination.
    input.value = '';
  };
  const fault = value?.fault ?? null;
  const faultText = value?.fault !== null && value?.fault !== undefined && value.path !== undefined
    ? value.fault === 'invalid-path' ? t('importWizard.source.invalidPathNamed', { path: value.path })
      : value.fault === 'duplicate-path' ? t('importWizard.source.duplicatePathNamed', { path: value.path }) : t(`importWizard.source.${FAULTS[value.fault]}`)
    : fault === null ? null : t(`importWizard.source.${FAULTS[fault]}`);
  const selected = value?.fault === null ? value.files : [];
  return <div className="flex min-w-0 flex-col gap-3">
    <DropTarget label={t('importWizard.source.drop')} acceptReferences={false} disabled={blocked}
      onDrop={(data) => drops.read(data, t('importWizard.source.drop'), (picked) => choose(picked, false))}>
      <div data-testid="import-drop-zone" className="flex min-w-0 flex-col gap-3 rounded-lg border border-dashed border-border-strong bg-bg-subtle p-4">
        <span className="text-sm text-text-muted">{t('importWizard.source.drop')}</span>
        <div className="flex flex-wrap gap-2">
          <Button icon={<FileUp aria-hidden />} disabled={blocked} onClick={() => files.current?.click()}>{t('importWizard.source.chooseFiles')}</Button>
          <Button icon={<FolderOpen aria-hidden />} disabled={blocked} onClick={() => folder.current?.click()}>{t('importWizard.source.chooseFolder')}</Button>
          <Button icon={<Archive aria-hidden />} disabled={blocked} onClick={() => archive.current?.click()}>{t('importWizard.source.chooseArchive')}</Button>
        </div>
      </div>
    </DropTarget>
    <input ref={files} data-testid="people-import-files" aria-label={t('importWizard.source.chooseFiles')} type="file" multiple hidden disabled={blocked} onChange={(event) => fromInput(event.currentTarget, false)} />
    <input ref={folder} data-testid="import-folder" aria-label={t('importWizard.source.chooseFolder')} type="file" multiple hidden disabled={blocked} {...{ webkitdirectory: '', directory: '' }} onChange={(event) => fromInput(event.currentTarget, false)} />
    <input ref={archive} data-testid="import-archive" aria-label={t('importWizard.source.chooseArchive')} type="file" accept=".zip,.tar,.tgz,.tar.gz,.tbz,.tbz2,.tar.bz2,.txz,.tar.xz,.tzst,.tar.zst" hidden disabled={blocked} onChange={(event) => fromInput(event.currentTarget, true)} />
    {drops.progress}
    {reading && <StateLine compact role="status" message={t('importWizard.dialog.checking')} />}
    {error !== null && <ErrorAlert error={error} />}
    {faultText !== null && <StateLine role="alert" tone="danger" compact message={faultText} />}
    {value?.fault === null && <>
      <p className="text-sm text-text-muted tabular-nums">{t('importWizard.source.total', { count: selected.length, number: formatNumber(selected.length, locale), size: formatBytes(value.bytes, locale) })}</p>
      <ul aria-label={t('importWizard.source.selected')} className="divide-y divide-border rounded-lg border border-border">{selected.slice(start, start + SOURCE_PAGE).map(({ file, path }) => <li key={path} className="flex min-h-(--row-height) min-w-0 items-baseline justify-between gap-3 px-3 py-2 text-sm">
        <span className="min-w-0 break-all" title={path}>{path}</span><ByteSize bytes={file.size} />
      </li>)}</ul>
      <ImportPagination first={start > 0} more={selected.length > start + SOURCE_PAGE} onFirst={() => setStart(0)} onMore={() => setStart((at) => at + SOURCE_PAGE)} />
    </>}
  </div>;
}
