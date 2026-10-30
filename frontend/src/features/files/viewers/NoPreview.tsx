/** Anything else: what it is, its size and its type, and Download.
 *
 * A file no viewer shows is not left without a way on: its name, its size, its type and the hash that
 * names its content are said, and it can be downloaded or opened as text.
 */
import { FileText } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useFormatContext } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { CopyButton } from '@/ui/CopyButton';
import { Button } from '@/ui/Button';

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
            {hash !== null && (
              <>
                <dt className="text-text-muted">{t('files.none.hash')}</dt>
                <dd className="flex items-center gap-1 font-mono text-xs wrap-anywhere">
                  <span className="min-w-0">{hash}</span>
                  <CopyButton value={hash} label={t('files.none.copyHash')} />
                </dd>
              </>
            )}
          </dl>
      <Button size="sm" className="self-start" icon={<FileText aria-hidden />} onClick={() => openAs('code')}>{t('files.none.openAsText')}</Button>
    </div>
  );
}
