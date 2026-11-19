/** A thread's links: labelled addresses in its heading that open where they point, edited by whoever may talk to its agent, and a pull request's address offered, never saved by itself.
 *
 * Orch fetches nothing of them: no preview, no state of a forge. A reader sees the list alone; somebody
 * who holds `thread.interact` edits it as one list and saves it in one act. A save that is refused keeps
 * the edits and says why. The address of a pull request the agent's last answer named is offered with
 * "Add link", which only puts it into the form. On a phone the links fold into one button.
 */
import { useMutation } from '@tanstack/react-query';
import { ExternalLink, Link2, Plus, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { saveThreadLinks } from '@/api/source';
import type { Thread, ThreadLink } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { threadWriter } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { Menu } from '@/ui/Menu';

import { linkFault, MOST_LINKS } from './words';

/** The links the server suggests for the thread (a pull request's address its agent gave), where it sends any. */
function suggestedOf(thread: Thread): ThreadLink[] {
  if (!('suggested_links' in thread) || !Array.isArray(thread.suggested_links)) return [];
  return (thread.suggested_links as unknown[]).flatMap((entry) => {
    if (typeof entry !== 'object' || entry === null) return [];
    const record = entry as Record<string, unknown>;
    return typeof record.url === 'string' && typeof record.label === 'string' ? [{ url: record.url, label: record.label }] : [];
  });
}

/** The heading's links, and Edit links… for whoever may change them. */
export function ThreadLinks({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const mayEdit = holds(thread.access, 'thread.interact') && !thread.archived;
  const links = thread.links;
  const suggested = suggestedOf(thread).filter((one) => !links.some((link) => link.url === one.url));
  if (links.length === 0 && !mayEdit) return null;
  return (
    <span className="inline-flex min-w-0 flex-wrap items-center gap-1" data-thread-links>
      <span className="hidden flex-wrap items-center gap-1 sm:inline-flex">
        {links.map((link) => (
          <a key={link.url} href={link.url} target="_blank" rel="noopener noreferrer" className="inline-flex max-w-48 items-center gap-1 rounded-full border border-border px-2 text-xs text-text hover:bg-bg-subtle" title={link.url}>
            <ExternalLink aria-hidden className="size-4 shrink-0" />
            <span className="truncate">{link.label === '' ? link.url : link.label}</span>
          </a>
        ))}
      </span>
      {links.length > 0 && (
        <span className="sm:hidden">
          <Menu
            label={t('source.links.list')}
            entries={links.map((link) => ({ id: link.url, label: link.label === '' ? link.url : link.label, icon: <ExternalLink aria-hidden />, onSelect: () => { window.open(link.url, '_blank', 'noopener,noreferrer'); } }))}
            trigger={<Button size="sm" variant="ghost" icon={<Link2 aria-hidden />}>{t('source.links.count', { count: links.length })}</Button>}
          />
        </span>
      )}
      {mayEdit && <IconButton size="sm" label={t('source.links.edit')} icon={<Link2 aria-hidden />} onClick={() => setEditing(true)} />}
      {mayEdit && suggested.length > 0 && !editing && (
        <Button size="sm" variant="ghost" icon={<Plus aria-hidden />} onClick={() => setEditing(true)}>
          {t('source.links.add')}
        </Button>
      )}
      {editing && <LinksDialog thread={thread} suggested={suggested} onClose={() => setEditing(false)} />}
    </span>
  );
}

function LinksDialog({ thread, suggested, onClose }: { thread: Thread; suggested: ThreadLink[]; onClose: () => void }) {
  const { t } = useTranslation();
  const [rows, setRows] = useState<ThreadLink[]>(() => thread.links.map((link) => ({ ...link })));
  const save = useMutation({
    mutationFn: () => saveThreadLinks(thread.id, rows.map((row) => ({ url: row.url.trim(), label: row.label.trim() }))),
    onSuccess: (links) => {
      threadWriter.apply(thread.id, { thread: { ...thread, links } });
      onClose();
    },
  });
  const faults = rows.map((row) => (row.url.trim() === '' ? t('source.links.fault.empty') : linkFault(t, row)));
  const offered = suggested.filter((one) => !rows.some((row) => row.url.trim() === one.url));
  const change = (index: number, part: Partial<ThreadLink>) => {
    save.reset();
    setRows((before) => before.map((row, at) => (at === index ? { ...row, ...part } : row)));
  };
  return (
    <Dialog
      open
      size="md"
      title={t('source.links.title')}
      description={t('source.links.lead')}
      onOpenChange={(next) => { if (!next) onClose(); }}
      footer={
        <>
          <Button onClick={onClose}>{t('common.cancel')}</Button>
          <Button action="save" variant="primary" pending={save.isPending} disabled={faults.some((fault) => fault !== null)} onClick={() => save.mutate()}>
            {t('source.links.save')}
          </Button>
        </>
      }
    >
      <form className="flex flex-col gap-3" onSubmit={(event) => { event.preventDefault(); if (faults.every((fault) => fault === null)) save.mutate(); }}>
        {rows.length === 0 && <EmptyState compact title={t('source.links.none')} />}
        {rows.map((row, index) => (
          <div key={index} className="flex flex-col gap-2 rounded-lg border border-border p-2 sm:flex-row sm:items-start">
            <Input label={t('source.links.label')} value={row.label} fieldClassName="sm:w-40" onChange={(event) => change(index, { label: event.target.value })} />
            <Input label={t('source.links.url')} value={row.url} mono type="url" inputMode="url" fieldClassName="min-w-0 flex-1" error={row.url.trim() === '' ? undefined : (faults[index] ?? undefined)} onChange={(event) => change(index, { url: event.target.value })} />
            <IconButton className="sm:mt-6" label={t('source.links.remove', { name: row.label === '' ? row.url : row.label })} icon={<X aria-hidden />} onClick={() => { save.reset(); setRows((before) => before.filter((_, at) => at !== index)); }} />
          </div>
        ))}
        <div className="flex flex-wrap gap-2">
          <Button size="sm" icon={<Plus aria-hidden />} disabled={rows.length >= MOST_LINKS} onClick={() => setRows((before) => [...before, { url: '', label: '' }])}>
            {t('source.links.another')}
          </Button>
          {offered.map((one) => (
            <Button key={one.url} size="sm" variant="ghost" icon={<Plus aria-hidden />} disabled={rows.length >= MOST_LINKS} onClick={() => setRows((before) => [...before, { ...one }])}>
              {t('source.links.addNamed', { name: one.label === '' ? one.url : one.label })}
            </Button>
          ))}
        </div>
        {save.error !== null && <ErrorAlert error={save.error} />}
      </form>
    </Dialog>
  );
}
