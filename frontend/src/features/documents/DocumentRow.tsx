/** The same document facts in recent lists and document directories: its title, the first line of what it is about, and who changed it when. */
import { Link } from '@tanstack/react-router';
import { Bot, FileText, MessageSquare } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { DocumentRef } from '@/generated/api-types';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { AuthorChip } from '@/ui/AuthorChip';
import { ContextMenu } from '@/ui/ContextMenu';
import { LockBadge } from '@/ui/LockBadge';
import type { MenuEntry } from '@/ui/Menu';
import { RelativeTime } from '@/ui/RelativeTime';

export function DocumentRow({ doc, here, where, me, name, actions, menu }: { doc: DocumentRef; here: string; where: boolean; me: string | null; name?: string; actions?: ReactNode; menu?: readonly MenuEntry[] }) {
  const { t } = useTranslation();
  const by = doc.updated_by;
  const mine = by !== null && 'id' in by && by.id === me;
  const changed = !mine && (doc.seen_seq === null || doc.seq > doc.seen_seq);
  const agent = 'agent' in doc.created_by;
  const row = (
    <li className="flex items-start gap-3 px-3 py-3" data-document-row={doc.id}>
      <span className="flex h-(--control-sm) shrink-0 items-center">{agent ? <Bot aria-hidden className="size-4 shrink-0 text-text-muted" /> : <FileText aria-hidden className="size-4 shrink-0 text-text-muted" />}</span>
      <div className="flex min-w-0 flex-1 flex-col">
        <span className="flex min-h-(--control-sm) items-center gap-2">
          <Link to="/doc/$documentId" params={{ documentId: doc.id }} className="truncate text-base font-medium text-text hover:underline focus-visible:outline-2 focus-visible:outline-focus">
            <span data-user-content>{name ?? doc.title}</span>
          </Link>
          {doc.page === 'restricted' && <LockBadge label={t('documents.access.private')} />}
          <AsAdminMark of={adminAccessOf(doc)?.place} responsive />
          {changed && <span role="img" aria-label={t('documents.list.changed')} className="size-2 shrink-0 rounded-full bg-accent" />}
        </span>
        {doc.description_line !== '' && <span data-document-line className="truncate text-sm text-text-muted">{doc.description_line}</span>}
        <span className="flex flex-wrap items-center gap-x-2 text-sm text-text-muted">
          {where && doc.node.id !== here && <span>{doc.node.name ?? t('documents.access.place')} ·</span>}
          {by !== null && <span className="inline-flex items-center gap-1">{t('agentIdentity.changedBy')} <AuthorChip author={by} /></span>}
          <RelativeTime at={doc.updated_at} />
          {agent && <span className="inline-flex items-center gap-1">· {t('agentIdentity.madeBy')} <AuthorChip author={doc.created_by} /></span>}
        </span>
      </div>
      {doc.open_discussions > 0 && (
        <span className="flex items-center gap-1 text-sm text-text-muted" aria-label={t('documents.list.discussions', { count: doc.open_discussions })}>
          <MessageSquare aria-hidden className="size-4" />
          {doc.open_discussions}
        </span>
      )}
      {actions}
    </li>
  );
  return menu === undefined ? row : <ContextMenu label={doc.title} entries={menu}>{row}</ContextMenu>;
}
