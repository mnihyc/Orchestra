/** What the viewer lacks to work with this thread, and whose it is, with Request access and Ask in chat.
 *
 * What stands where the composer would, for somebody who may watch a thread and not talk to its agent:
 * one line for each thing they lack, in their own terms (to talk to its agent; the model and the effort it
 * runs on), whose it is, and whom to ask. Something nobody can give says so and is left out of the asking.
 * On a person's own machine one more line says that, as the thread decides, only its owner works with it
 * until they name somebody. The refusal is the server's own list of what is missing.
 */
import { Eye } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useCallback, useState } from 'react';
import { AccessRequest } from '@/features/access/AccessRequest';
import { effortLabel, modelLabel } from '@/features/change/modelLabel';
import { open as openView } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { reloadThread } from '@/realtime/live';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';

import type { MissingAccess, Thread } from '@/generated/api-types';

function Line({ entry, thread }: { entry: MissingAccess; thread: Thread }) {
  const { t } = useTranslation();
  const name = entry.object.name ?? t('thread.watching.thisThread');
  const owner = entry.object.owner?.display_name ?? thread.owner.display_name;
  if (entry.closed !== null) return <li>{t('thread.watching.closed', { name, owner })}</li>;
  if (entry.object.kind === 'provider' || entry.options !== null) {
    const model = modelLabel(thread.model ?? null, [], thread.model_display ?? null, t);
    const effort = effortLabel(entry.options?.efforts?.[0] ?? thread.effort ?? null, t);
    return <li>{effort === null ? t('thread.watching.use', { name, model }) : t('thread.watching.useAt', { name, model, effort })}</li>;
  }
  return <li>{t('thread.watching.talk', { name })}</li>;
}

export function WatchingCard({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const missing = thread.input_access.missing;
  const [requesting, setRequesting] = useState(false);
  const refresh = useCallback(() => { reloadThread(thread.id); }, [thread.id]);
  const ask = missing.flatMap((entry) => entry.ask).filter((person, index, all) => all.findIndex((other) => other.id === person.id) === index);
  const person = thread.environment?.placement === 'connected' && missing.some((entry) => entry.object.kind === 'thread');
  const requestable = missing.some((entry) => entry.closed === null);
  return (
    <section aria-label={t('thread.watching.label')} data-watching className="flex flex-col gap-2 rounded-lg border border-border bg-bg-subtle px-4 py-3 text-sm text-text">
      <p className="flex items-center gap-2 font-medium">
        <Eye className="text-text-muted" aria-hidden />
        {t('thread.watching.title')}
      </p>
      <ul className="flex list-disc flex-col gap-1 pl-6 text-text-muted">
        {missing.map((entry, index) => (
          <Line key={`${entry.object.id}.${index}`} entry={entry} thread={thread} />
        ))}
      </ul>
      {person && <p className="text-text-muted">{t('thread.watching.person', { owner: thread.owner.display_name, thread: thread.name })}</p>}
      <p className="text-text-muted">{ask.length > 0 ? t('thread.watching.askWho', { names: ask.map((entry) => entry.display_name).join(', ') }) : t('thread.watching.nobody')}</p>
      <div className="flex flex-wrap gap-2">
        {requestable && <Button size="sm" onClick={() => setRequesting(true)}>{t('access.request.title')}</Button>}
        <Button size="sm" onClick={() => { const workbench = useWorkbench.getState(); workbench.change((layout) => openView(layout, 'chat', 'right')); workbench.setOverlay('right'); }}>{t('providerPolicy.askInChat')}</Button>
      </div>
      <Dialog open={requesting} onOpenChange={setRequesting} title={t('access.request.title')}>
        <AccessRequest missing={missing} objectId={thread.id} forThread={thread.id} fixed onChanged={refresh} />
      </Dialog>
    </section>
  );
}
