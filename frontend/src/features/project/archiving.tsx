/** An archive is made at once; only work it stops is said first, in one line, with the threads on Show. */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { NodeArchivePlan } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ReviewPlace } from './ReviewConsequences';

type Work = NodeArchivePlan['work'][number];

/** The threads an archive stops something in: running turns, waiting inputs or approvals; unknown counts are not guessed. */
export function stoppedWork(plans: readonly NodeArchivePlan[]): Work[] {
  if (plans.some((plan) => !plan.archiving)) return [];
  return plans.flatMap((plan) => plan.work).filter((item) => (item.turns ?? 0) + (item.inputs ?? 0) + (item.approvals ?? 0) > 0);
}

export function StopLine({ work }: { work: readonly Work[] }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  if (work.length === 0) return null;
  return <div className="flex min-w-0 flex-col gap-2 text-sm">
    <p className="flex flex-wrap items-center gap-2">
      <span className="text-warning-text">{t('places.archive.stops', { count: work.length })}</span>
      <Button size="sm" variant="ghost" aria-expanded={open} onClick={() => setOpen(!open)}>{t(open ? 'actions.hide.label' : 'actions.show.label')}</Button>
    </p>
    {open && <ul className="flex flex-col gap-1">{work.map((item) => <li key={item.thread.id}><ReviewPlace place={item.thread} /></li>)}</ul>}
  </div>;
}
