/** The Comments view of a document, in the right bar beside Chat: the discussions of the document in view, and what opens it.
 *
 * The workbench offers a Comments view on every document and shows a note in it until the part that
 * builds it registers the real one. Registering is replacing: the same name, the real body.
 */
import { useViewInstance } from '@/features/workbench/instance';
import { useParams } from '@tanstack/react-router';
import { MessageSquareQuote } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { CommentRecoveryNotice, CommentsPanel } from '@/features/documents/Comments';
import { revealView } from '@/features/workbench/openView';
import { reloadDocument, useDocumentFeed } from '@/realtime/live';
import { useDocumentState } from '@/realtime/stores/document';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';

/** The pane owns its selected document reader even when that document page is closed. */
export function CommentsView() {
  const { t } = useTranslation();
  const params = useParams({ strict: false });
  const instance = useViewInstance();
  const documentId = instance === null ? params.documentId : instance.target?.kind === 'document' ? instance.target.id : undefined;
  useDocumentFeed(documentId ?? null);
  const state = useDocumentState(documentId ?? null);
  const displayed = state.ended === null && state.error === null ? state.document : null;
  return <div className="flex h-full min-h-0 flex-col">
    <CommentRecoveryNotice displayedDocument={displayed?.id} />
    <div className="min-h-0 flex-1">{documentId === undefined
      ? <EmptyState compact className="h-full" icon={<MessageSquareQuote aria-hidden />} title={t('views.comments.title')} description={t('views.comments.about')} />
      : state.ended !== null
        ? <EmptyState compact className="h-full" icon={<MessageSquareQuote aria-hidden />} title={t(state.ended === 'deleted' ? 'documents.page.endedDeleted' : 'documents.page.endedRevoked')} />
        : state.error !== null
          ? <ErrorState compact className="h-full" error={state.error} onRetry={() => reloadDocument(documentId)} />
          : displayed === null
            ? <EmptyState compact className="h-full" icon={<MessageSquareQuote aria-hidden />} title={t('documents.comments.panel')} description={t('common.loading')} />
            : <CommentsPanel key={displayed.id} doc={displayed} />}</div>
  </div>;
}

/** Brings the Comments view to the front of the right bar, opening the bar; where the bar lies over the page, the sheet is opened. */
export function revealComments(_beside: boolean): void { revealView('comments'); }

/** Uses the same visible desktop/phone opening path as Comments. */
export function revealOutline(_beside: boolean): void { revealView('outline'); }
