/** Anything else: what it is, its size and its type, and Download.
 *
 * A file no viewer shows is not left without a way on: its size and its type are said, and it can be
 * downloaded or opened as text. The hash that names its content is one Copy ID act away, never a line.
 */
import { FileText } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useFormatContext } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { Button } from '@/ui/Button';
import { CopyId } from '@/ui/CopyId';

import type { ViewerProps } from '../viewers';
import { FileState } from '../FilePreview';

export default function NoPreview({ head, openAs }: ViewerProps) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const hash = head.brought.state === 'ready' ? head.brought.hash : head.stat.hash;
  return (
    <div className="flex min-w-0 flex-col gap-3 p-3">
      <FileState state="unavailable" />
          <dl className="grid w-full grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-left text-sm">
            <dt className="text-text-muted">{t('files.none.size')}</dt>
            <dd className="tabular-nums">{formatBytes(head.stat.size, locale)}</dd>
            <dt className="text-text-muted">{t('files.none.type')}</dt>
            <dd className="wrap-anywhere">{head.stat.media_type}</dd>
          </dl>
      <div className="flex items-center gap-1 self-start">
        <Button size="sm" icon={<FileText aria-hidden />} onClick={() => openAs('code')}>{t('files.none.openAsText')}</Button>
        {hash !== null && <CopyId value={hash} />}
      </div>
    </div>
  );
}
