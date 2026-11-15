/** Draft persistence is a small mark in the control row, announced as status and explained on hover. */
import { CircleAlert, LoaderCircle } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { type DraftStatus, persistComposerDraft, useComposerStore } from '@/features/composer/Draft';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Popover } from '@/ui/Popover';
import { Tooltip } from '@/ui/Tooltip';

export function DraftMark({ status, threadId, storeKey }: { status: DraftStatus; threadId?: string | null | undefined; storeKey?: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const key = storeKey ?? threadId ?? '';
  const saved = useComposerStore((state) => state.draftSaves[key]);
  const words = status === 'saving' ? t('composer.draft.saving') : status === 'failed' ? t('composer.draft.failed') : undefined;
  const failure = status === 'failed' && threadId !== undefined && threadId !== null;
  return <span data-draft-mark className="inline-flex size-(--control-sm) shrink-0 items-center justify-center">
    {failure ? <>
      <span role="status" className="sr-only">{words}</span>
      <Popover label={t('composer.draft.failed')} open={open} onOpenChange={setOpen} side="top"
        trigger={<IconButton size="sm" label={t('composer.draft.failed')} icon={<CircleAlert aria-hidden />} className="text-danger-text" />}>
        <ErrorAlert error={saved?.error} action={<Button size="sm" onClick={() => {
          const current = useComposerStore.getState().draftSaves[key];
          if (current?.status !== 'failed' || current.threadId !== threadId) return;
          void persistComposerDraft(current.threadId, key).catch(() => undefined);
        }}>{t('common.tryAgain')}</Button>} />
      </Popover>
    </> : words !== undefined && <Tooltip content={words}><span role="status" className={status === 'failed' ? 'inline-flex text-danger-text' : 'inline-flex text-text-muted'}>
      {status === 'saving' ? <LoaderCircle className="size-4 animate-spin" aria-hidden /> : <CircleAlert className="size-4" aria-hidden />}
      <span className="sr-only">{words}</span>
    </span></Tooltip>}
  </span>;
}
