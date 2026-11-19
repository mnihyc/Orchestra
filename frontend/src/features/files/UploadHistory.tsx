/** Upload history names the actor, files, destination, measured bytes and outcome in one compact line. */
import { useTranslation } from 'react-i18next';
import { useState } from 'react';

import { TypedChip } from '@/features/people/NamedChip';
import type { HistoryLine } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { UserChip } from '@/ui/UserChip';
import { Popover } from '@/ui/Popover';
import { Button } from '@/ui/Button';
import { VirtualList } from '@/ui/VirtualList';

/** Keep a history line compact while its complete list remains available in its record. */
const VISIBLE_FILE_NAMES = 2;
/** A compact file row starts at the same density as the workspace tree; measured rows correct this estimate. */
const ESTIMATED_FILE_ROW_PX = 36;

export function UploadHistory({ line }: { line: HistoryLine }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const [open, setOpen] = useState(false);
  const upload = line.upload;
  if (upload === undefined || upload === null) return null;
  const rest = upload.files.length - VISIBLE_FILE_NAMES;
  return <span className="flex min-w-0 flex-wrap items-baseline gap-x-1 gap-y-1 text-sm">
    {line.actor !== null && 'display_name' in line.actor ? <UserChip user={line.actor} /> : <span className="text-text-muted">{t('uploads.history.unknownPerson')}</span>}
    <span>{t(`uploads.history.${upload.outcome}`)}</span>
    {upload.files.slice(0, VISIBLE_FILE_NAMES).map((file, index) => <span key={`${file.name}-${index}`} className="inline-flex max-w-full items-baseline gap-1">
      {index > 0 && <span>{t('uploads.history.separator')}</span>}
      {file.path === null ? <span className="truncate text-text-muted" title={file.name}>{file.name}</span> : <TypedChip reference={{ kind: 'environment_file', id: upload.environment_id, path: file.path }} />}
    </span>)}
    {rest > 0 && <Popover label={t('uploads.history.files')} open={open} onOpenChange={setOpen}
      trigger={<Button size="sm" variant="ghost">{t('uploads.history.more', { number: formatNumber(rest, locale) })}</Button>} className="w-80 max-w-[calc(100vw-1rem)]">
      <div style={{ height: 'min(16rem, max(0px, calc(var(--radix-popover-content-available-height) - 2rem)))' }}>
        <VirtualList className="h-full min-h-0 overflow-y-auto" count={upload.files.length} startAtEnd={false} follow={false} label={t('uploads.history.files')}
          keyOf={(index) => `${index}-${upload.files[index]?.path ?? upload.files[index]?.name ?? ''}`} estimate={() => ESTIMATED_FILE_ROW_PX}
          render={(index) => { const file = upload.files[index]; return file === undefined ? null : <div className="flex min-w-0 items-center gap-2 py-1 text-sm">
            <span className="min-w-0 flex-1">{file.path === null ? <span className="block truncate text-text-muted" title={file.name}>{file.name}</span> : <TypedChip reference={{ kind: 'environment_file', id: upload.environment_id, path: file.path }} />}</span>
            <span className="shrink-0 text-text-muted tabular-nums">{file.received === null ? '–' : formatBytes(file.received, locale)} / {formatBytes(file.size, locale)}</span>
          </div>; }} />
      </div>
    </Popover>}
    <span>{t('uploads.history.to', { path: upload.folder === '' ? '/' : upload.folder })}</span>
    <span className="text-text-muted tabular-nums">· {upload.bytes === null ? '–' : formatBytes(upload.bytes, locale)}</span>
    {upload.replaced > 0 && <span className="text-text-muted">· {t('uploads.history.replaced', { number: formatNumber(upload.replaced, locale) })}</span>}
  </span>;
}
