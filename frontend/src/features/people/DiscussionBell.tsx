/** A document's two-choice bell, with pending and refused changes kept beside the control. */
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceCopy } from '@/i18n/choices';
import { setDiscussionLevel } from '@/api/people';
import { ErrorAlert } from '@/ui/ErrorState';
import { BellTrigger } from '@/features/notifications/BellTrigger';
import { Menu } from '@/ui/Menu';
import { Popover } from '@/ui/Popover';
import type { DocumentDiscussionLevel as DiscussionLevel, Document as HeardDocument } from '@/generated/api-types';

export function DiscussionBell({ doc }: { doc: HeardDocument }) {
  const { t } = useTranslation();
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const refused = useRef(false);
  const [served, setServed] = useState(doc.discussion_level);
  const [level, setLevel] = useState<DiscussionLevel>(doc.discussion_level);
  if (doc.discussion_level !== served) { setServed(doc.discussion_level); setLevel(doc.discussion_level); }
  const choose = async (next: DiscussionLevel) => {
    if (pending || next === level) return;
    setPending(true); refused.current = false; setError(null);
    try {
      await setDiscussionLevel(doc.id, next);
      setLevel(next);
    } catch (failure) { refused.current = true; setError(failure); }
    finally { setPending(false); }
  };
  return <>
    <Popover anchor label={t('people.discussionBell')} open={error !== null} onOpenChange={(open) => { if (!open) { refused.current = false; setError(null); } }}
      onCloseAutoFocus={(event) => { event.preventDefault(); trigger.current?.focus(); }} className="w-64"
      trigger={<span className="inline-flex"><Menu label={t('people.discussionBell')} align="end" onCloseAutoFocus={(event) => { event.preventDefault(); if (!refused.current) trigger.current?.focus(); }} trigger={<BellTrigger ref={trigger} label={t('people.discussionBell')} scopeLabel={t('notificationBell.scope.discussion')} level={level} pending={pending} />} entries={[
      { type: 'radio', id: 'discussions', label: t('people.discussionBell'), value: level, onValueChange: (value) => void choose(value as DiscussionLevel), options: [
        { value: 'all', ...choiceCopy(t, 'discussionLevel.all') }, { value: 'involved', ...choiceCopy(t, 'discussionLevel.involved') },
      ] },
    ]} /></span>}>
      {error !== null && <ErrorAlert error={error} />}
    </Popover>
    {pending && <span role="status" className="sr-only">{t('common.loading')}</span>}
  </>;
}
