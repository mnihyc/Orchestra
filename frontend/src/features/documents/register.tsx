/** Puts the real Documents view into the workbench, in place of the note that said it was not built.
 *
 * A view is registered under its name, and a view registered again takes the place of the one before: this is how the part that
 * builds a view replaces its note. It is imported once, where the shell is put together; the list itself is loaded when the view is first drawn.
 */
import { FileText, MessageSquareQuote } from 'lucide-react';
import { lazy, Suspense } from 'react';

import type { ViewProps } from '@/features/workbench/views';
import { registerView } from '@/features/workbench/views';
import { SkeletonRows } from '@/ui/Skeleton';

const Panel = lazy(() => import('@/features/documents/PlaceDocuments').then((module) => ({ default: module.PlaceDocuments })));

function DocumentsView(props: ViewProps) {
  return (
    <Suspense fallback={<SkeletonRows rows={4} className="p-4" />}>
      <Panel {...props} />
    </Suspense>
  );
}

registerView({
  name: 'documents',
  title: 'views.documents.title',
  about: 'views.documents.about',
  icon: FileText,
  // A topic already has its Documents page tab; its side bar must not repeat it.
  screens: ['org', 'project', 'thread'],
  area: 'right',
  body: DocumentsView,
});

// A restored Comments pane must exist before any document page has been opened.
const Comments = lazy(() => import('@/features/documents/views').then((module) => ({ default: module.CommentsView })));
function DocumentCommentsView() {
  return <Suspense fallback={<SkeletonRows rows={4} className="p-4" />}><Comments /></Suspense>;
}
registerView({
  name: 'comments', title: 'views.comments.title', about: 'views.comments.about',
  icon: MessageSquareQuote, screens: ['document'], area: 'right', body: DocumentCommentsView,
});
