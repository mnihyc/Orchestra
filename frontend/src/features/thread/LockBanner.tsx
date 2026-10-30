/** The switch, move or update that holds the thread: its percentage, its phase and the way back to its wizard.
 *
 * What holds the thread while a switch, a move or an update goes on: what it is and how far it has
 * come, as words and as a bar that never moves back. The composer stays: what is sent meanwhile is
 * delivered when the job is done.
 */
import { useTranslation } from 'react-i18next';

import { openChange } from '@/features/change/target';
import type { Thread } from '@/generated/api-types';
import { Banner } from '@/ui/Banner';
import { Button } from '@/ui/Button';
import { Meter } from '@/ui/Meter';
import { waitingWords } from '@/features/timeline/waiting';

export function LockBanner({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const lock = thread.lock;
  if (lock === null) return null;
  const words = waitingWords(thread, t) ?? '';
  return (
    <Banner
      tone="info"
      className="flex-wrap rounded-md border"
      action={
        <Button size="sm" onClick={() => openChange({ kind: 'thread', id: thread.id, name: thread.name }, { kind: 'progress', job: lock.id })}>
          {t('change.banner.progress')}
        </Button>
      }
    >
      <span data-lock={lock.kind}>{t('thread.lock.banner', { what: words.replace(/ · \d+ ?%$/, ''), percent: lock.percent })}</span>
      <Meter look="progress" value={lock.percent} label={t('thread.lock.progress')} className="mt-1" />
    </Banner>
  );
}
