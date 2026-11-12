/** The native missing access beside a viewer's disabled message controls. */
import { useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { AccessRequest } from '@/features/access/AccessRequest';
import { effortLabel, modelLabel } from '@/features/change/modelLabel';
import type { Thread } from '@/generated/api-types';
import { reloadThread } from '@/realtime/live';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';

export function ViewerAccessLine({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const [requesting, setRequesting] = useState(false);
  const read = useThreadReadRef(thread.id);
  const refresh = useCallback(() => reloadThread(read), [read]);
  const missing = thread.input_access.missing;
  const requestable = missing.filter((entry) => entry.closed === null && (entry.ask.length > 0 || entry.ask_more > 0));
  const ask = [...new Map(requestable.flatMap((entry) => entry.ask).map((person) => [person.id, person])).values()];
  const person = thread.environment?.placement === 'connected' && missing.some((entry) => entry.object.kind === 'thread');
  const access = missing.map((entry) => {
    const name = entry.object.name ?? t('thread.watching.thisThread');
    if (entry.object.kind !== 'provider' && entry.options === null) return t('thread.watching.talk', { name });
    const model = modelLabel(thread.model ?? null, [], thread.model_display ?? null, t);
    const effort = effortLabel(entry.options?.efforts?.[0] ?? thread.effort ?? null, t);
    return effort === null ? t('thread.watching.use', { name, model }) : t('thread.watching.useAt', { name, model, effort });
  });
  return <div role="group" aria-label={t('thread.watching.label')} data-viewer-access className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-xs text-text-muted">
    {access.length > 0 && <span>{t('thread.watching.missing', { access: access.join(' · ') })}</span>}
    <span>{ask.length > 0 ? t('thread.watching.askWho', { names: ask.map((entry) => entry.display_name).join(', ') })
      : requestable.length > 0 ? t('thread.watching.askOthers')
        : person ? t('thread.watching.person', { owner: thread.owner.display_name, thread: thread.name }) : t('thread.watching.nobody')}</span>
    {requestable.length > 0 && <Button size="sm" variant="ghost" onClick={() => setRequesting(true)}>{t('access.request.title')}</Button>}
    <Dialog open={requesting && requestable.length > 0} onOpenChange={setRequesting} title={t('access.request.title')}>
      <AccessRequest missing={requestable} objectId={thread.id} forThread={thread.id} fixed onChanged={refresh} />
    </Dialog>
  </div>;
}
