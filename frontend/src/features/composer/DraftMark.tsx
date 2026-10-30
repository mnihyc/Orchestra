/** Draft persistence is a small mark in the control row, announced as status and explained on hover. */
import { CircleAlert, LoaderCircle } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { DraftStatus } from '@/features/composer/Draft';
import { Tooltip } from '@/ui/Tooltip';

export function DraftMark({ status }: { status: DraftStatus }) {
  const { t } = useTranslation();
  const words = status === 'saving' ? t('composer.draft.saving') : status === 'failed' ? t('composer.draft.failed') : undefined;
  return <span data-draft-mark className="inline-flex h-(--control-sm) w-3 shrink-0 items-center justify-center">
    {words !== undefined && <Tooltip content={words}><span role="status" className={status === 'failed' ? 'inline-flex text-danger-text' : 'inline-flex text-text-muted'}>
      {status === 'saving' ? <LoaderCircle className="size-4 animate-spin" aria-hidden /> : <CircleAlert className="size-4" aria-hidden />}
      <span className="sr-only">{words}</span>
    </span></Tooltip>}
  </span>;
}
