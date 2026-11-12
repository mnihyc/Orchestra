/** A written description with its existing read and edit controls. */
import { useMutation } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { updateNode } from '@/api/nodes';
import type { Thread } from '@/generated/api-types';
import { threadWriter } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Markdown } from '@/ui/Markdown';
import { Textarea } from '@/ui/Textarea';

/** The first line of a description. */
const firstLine = (text: string): string => text.split('\n').find((line) => line.trim() !== '')?.trim() ?? '';

export function DescriptionLine({ thread, open, onToggle, mayEdit }: { thread: Pick<Thread, 'description'>; open: boolean; onToggle: () => void; mayEdit: boolean }) {
  const { t } = useTranslation();
  const line = firstLine(thread.description);
  if (line === '') {
    return mayEdit ? (
      <button type="button" onClick={onToggle} className="rounded-md text-sm whitespace-nowrap text-text-subtle hover:text-text hover:underline">
        {t('thread.description.add')}
      </button>
    ) : null;
  }
  return (
      <button
        type="button"
        aria-expanded={open}
        aria-label={open ? t('thread.description.collapse') : t('thread.description.expand')}
        onClick={onToggle}
        className="flex min-w-0 items-center gap-1 rounded-md text-left text-sm text-text-subtle hover:text-text"
      >
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
