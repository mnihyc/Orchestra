/** Words that stayed in the thread when their queued input was cancelled. */
import { useMutation } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { captureThreadRead } from '@/api/threadRead';
import { fetchItems } from '@/api/threads';
import { useTimelineContext } from '@/features/timeline/context';
import { Part, PERSON_BLOCK } from '@/features/timeline/items/UserMessage';
import type { NativeInvocation, RetainedInput, RetainedInputContinuation } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { copyableParts } from '@/lib/content';
import { formatExact } from '@/lib/time';
import { threadWriter } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadReceipt } from '@/realtime/threadScope';
import { Button } from '@/ui/Button';
import { CopyButton } from '@/ui/CopyButton';
import { ErrorAlert } from '@/ui/ErrorState';
import { FactDisclosure } from '@/ui/FactDisclosure';
import { TimeOfDay } from '@/ui/TimeOfDay';
import { UserChip } from '@/ui/UserChip';

function NativeAction({ action }: { action: NativeInvocation }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return <details onToggle={(event) => setOpen(event.currentTarget.open)}>
    <summary className="cursor-pointer text-sm">{t('retainedMessage.nativeAction')} · {action.action_id}</summary>
    {open && action.arguments !== undefined && action.arguments !== null &&
      <pre className="whitespace-pre-wrap wrap-anywhere">{JSON.stringify(action.arguments, null, 2)}</pre>}
  </details>;
}

export function RetainedMessage({ input }: { input: RetainedInput }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const { threadId } = useTimelineContext();
  const { author, outcome, native_action: action } = input;
  const words = copyableParts(input.parts);
  return <div className="flex flex-col items-end gap-1" data-retained-input={input.id}>
    {author !== null && <UserChip user={author} size="row" thing={{ kind: 'thread', id: threadId }} />}
    <div className={PERSON_BLOCK}>
      {input.parts.map((part, index) => <Part key={index} part={part} owner={author ?? undefined} updatedAt={input.submitted_at} />)}
      {action !== undefined && action !== null && <NativeAction action={action} />}
    </div>
    <div className="flex flex-wrap items-center justify-end gap-x-2 text-xs text-text-subtle">
      <TimeOfDay at={input.submitted_at} />
      <FactDisclosure fragment={outcome?.cause === 'access_changed' ? t('retainedMessage.accessChanged') : t('retainedMessage.cancelled')}
        groups={[{ key: 'cancellation', kind: 'recorded_outcome', facts: [
          { key: 'cause', label: t('common.reason'), value: outcome && t(`retainedMessage.reasons.${outcome.cause}`) },
          { key: 'at', label: t('retainedMessage.at'), value: outcome && <time dateTime={outcome.at}>{formatExact(outcome.at, format)}</time> },
          ...(outcome?.by ? [{ key: 'by', label: t('retainedMessage.by'), value: <UserChip user={outcome.by} size="row" thing={{ kind: 'thread', id: threadId }} /> }] : []),
        ] }]} />
      {words.trim() !== '' && <CopyButton value={words} label={t('timeline.actions.copy')} />}
    </div>
  </div>;
}

export function RetainedMessagePage({ group }: { group: RetainedInputContinuation }) {
  const { t } = useTranslation();
  const { threadId } = useTimelineContext();
  const read = useThreadReadRef(threadId);
  const request = useMutation({ mutationFn: async () => {
    const captured = captureThreadRead(read);
    const page = await fetchItems(read, { retained_inputs_cursor: group.cursor });
    threadWriter.retainedPage(read, group, page, threadReceipt(page) ?? captured);
  } });
  return <div className="flex flex-col items-end">
    <Button size="sm" variant="ghost" pending={request.isPending} onClick={() => request.mutate()}>{t('retainedMessage.more')}</Button>
    {request.error !== null && <ErrorAlert error={request.error} />}
  </div>;
}
