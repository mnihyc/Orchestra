/** An explicit request opens the thread's existing counted catch-up card, with no answer summary or model call. */
import { useTranslation } from 'react-i18next';

import { RequestedCatchupCard } from '@/features/timeline/Catchup';
import { Dialog } from '@/ui/Dialog';

export function CatchupRequest({ threadId, open, onOpenChange }: { threadId: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const { t } = useTranslation();
  return <Dialog open={open} onOpenChange={onOpenChange} title={t('timeline.catchup.title')}><RequestedCatchupCard threadId={threadId} /></Dialog>;
}
