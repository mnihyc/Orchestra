/** The answer to a plan an agent proposes: Proceed, or Keep planning with words of one's own.
 *
 * A plan can arrive as a request waiting for an answer or as the result of a planning turn.
 * Keep planning opens a box for what to change and sends it from there.
 */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { composerWriter } from '@/features/composer/Draft';
import { Hint } from '@/features/help/Hint';
import type { Approval } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Textarea } from '@/ui/Textarea';

/** Proceeding ends planning, so the composer's Plan switch of that thread goes off; keeping planning keeps it on. */
export function planningAfter(approval: Pick<Approval, 'thread_id'>, on: boolean): void {
  if (approval.thread_id !== null) composerWriter.setPlanning(approval.thread_id, on);
}

export function PlanAnswer({ disabled, pending, onProceed, onKeepPlanning }: {
  disabled: boolean;
  pending: 'proceed' | 'keep' | null;
  onProceed: () => void;
  onKeepPlanning: (words: string) => void;
}) {
  const { t } = useTranslation();
  const [words, setWords] = useState<string | null>(null);
  const writing = words !== null;
  return (
    <div className="flex flex-col gap-2" data-plan-answer>
      {writing && <Textarea label={t('plan.words')} rows={2} value={words} disabled={disabled} autoFocus
        onChange={(event) => setWords(event.target.value)} fieldClassName="w-full" />}
      <div className="flex flex-wrap items-center gap-2">
        <Hint id="plan-answer" popover />
        <Button variant="primary" size="md" disabled={disabled || pending !== null} pending={pending === 'proceed'} onClick={onProceed} data-plan-proceed>
          {t('plan.proceed')}
        </Button>
        <Button variant="secondary" size="md" disabled={disabled || pending !== null || (writing && words.trim() === '')} pending={pending === 'keep'}
          onClick={() => (writing ? onKeepPlanning(words.trim()) : setWords(''))} data-plan-keep>
          {t('plan.keepPlanning')}
        </Button>
        {writing && <Button variant="ghost" size="md" disabled={pending !== null} onClick={() => setWords(null)}>{t('common.cancel')}</Button>}
      </div>
    </div>
  );
}
