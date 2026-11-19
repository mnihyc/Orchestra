/** Manual or automatic progress text with override controls.
 *
 * The thread's description, in quiet type beside its title: its first line, or "Add a description" for
 * whoever may set it. When automation keeps it, it is the live progress of the work: a small glyph goes
 * before it, it changes in place, and its tooltip says when it was updated. Pressing it shows the whole
 * description as Markdown and lets whoever may change it do so in place; a person's text replaces the
 * automatic one, and the automatic one can be brought back, or refreshed, from the thread's menu.
 */
import { useMutation } from '@tanstack/react-query';
import { Radio } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { updateNode } from '@/api/nodes';
import { useFormatContext } from '@/lib/appearance';
import type { Thread } from '@/generated/api-types';
import { formatRelative } from '@/lib/time';
import { threadWriter } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Markdown } from '@/ui/Markdown';
import { Textarea } from '@/ui/Textarea';
import { toast } from '@/ui/Toast';
import { Tooltip } from '@/ui/Tooltip';

/** The first line of a description. */
const firstLine = (text: string): string => text.split('\n').find((line) => line.trim() !== '')?.trim() ?? '';

export function DescriptionLine({ thread, open, onToggle, mayEdit }: { thread: Pick<Thread, 'description' | 'description_source' | 'auto_updated_at'>; open: boolean; onToggle: () => void; mayEdit: boolean }) {
  const { t } = useTranslation();
  const context = useFormatContext();
  const line = firstLine(thread.description);
  const automatic = thread.description_source === 'auto';
  if (line === '') {
    return mayEdit ? (
      <button type="button" onClick={onToggle} className="rounded-md text-sm whitespace-nowrap text-text-subtle hover:text-text hover:underline">
        {t('thread.description.add')}
      </button>
    ) : null;
  }
  const when = thread.auto_updated_at === null ? null : formatRelative(thread.auto_updated_at, context);
  return (
      <button
        type="button"
        aria-expanded={open}
        aria-label={open ? t('thread.description.collapse') : t('thread.description.expand')}
        aria-description={automatic && when !== null ? t('thread.description.updated', { when }) : undefined}
        onClick={onToggle}
        className="flex min-w-0 items-center gap-1 rounded-md text-left text-sm text-text-subtle hover:text-text"
      >
        {automatic && <Tooltip content={when === null ? undefined : t('thread.description.updated', { when })}><span className="inline-flex shrink-0"><Radio className="text-accent" aria-hidden /></span></Tooltip>}
        {/* Changing in place, with a short cross-fade: the new line is drawn anew under its own key. */}
        <span key={line} data-overflow-text={thread.description} className="min-w-0 animate-rise-in truncate">
          {line}
        </span>
      </button>
  );
}

export function DescriptionPanel({ thread, mayEdit, onClose, initialEdit = false }: { thread: Thread; mayEdit: boolean; onClose: () => void; initialEdit?: boolean }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(mayEdit && (initialEdit || thread.description.trim() === ''));
  const [text, setText] = useState(thread.description);
  const save = useMutation({
    mutationFn: () => updateNode(thread.id, { description: text, expected_revision: thread.revision }),
    onSuccess: (node) => {
      if (node.kind === 'thread') threadWriter.apply(thread.id, { thread: node });
      if (thread.description_source === 'auto') toast({ tone: 'info', title: t('thread.description.replaces') });
      setEditing(false);
      if (initialEdit) onClose();
    },
  });
  return (
    <div className="flex max-w-prose flex-col gap-2 rounded-lg border border-border bg-bg-subtle px-3 py-2">
      {editing ? (
        <form
          className="flex flex-col gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            save.mutate();
          }}
        >
          <Textarea label={t('thread.description.field')} hideLabel rows={3} value={text} onChange={(event) => setText(event.target.value)} />
          {save.error !== null && <ErrorAlert error={save.error} />}
          <div className="flex gap-2">
            <Button type="submit" size="sm" variant="primary" pending={save.isPending}>
              {t('thread.description.save')}
            </Button>
            <Button
              size="sm"
              variant="ghost"
              disabled={save.isPending}
              onClick={() => {
                setText(thread.description);
                setEditing(false);
                if (thread.description.trim() === '') onClose();
              }}
            >
              {t('thread.description.cancel')}
            </Button>
          </div>
        </form>
      ) : (
        <>
          <Markdown text={thread.description} compact />
          <div className="flex gap-2">
            {mayEdit && (
              <Button size="sm" variant="ghost" onClick={() => setEditing(true)}>
                {t('thread.description.edit')}
              </Button>
            )}
            <Button size="sm" variant="ghost" onClick={onClose}>
              {t('thread.description.collapse')}
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
