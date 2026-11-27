/** A first-use dialog keeps its own loading and chunk failure inside the existing frame. */
import { type ReactNode, Suspense } from 'react';
import { ContentFailure } from '@/ui/ContentBoundary';
import { Dialog } from '@/ui/Dialog';
import { ErrorBoundary } from '@/ui/ErrorBoundary';
import { SkeletonRows } from '@/ui/Skeleton';

export function DeferredDialog({ open, onClose, title, resetKey, children }: { open: boolean; onClose: () => void; title: string; resetKey?: unknown; children: ReactNode }) {
  const changed = (next: boolean) => { if (!next) onClose(); };
  return <ErrorBoundary resetKey={resetKey} fallback={(_error, retry) => <Dialog open={open} onOpenChange={changed} title={title}>
    <ContentFailure retry={retry} />
  </Dialog>}>
    <Suspense fallback={<Dialog open={open} onOpenChange={changed} title={title}><SkeletonRows rows={3} /></Dialog>}>
      {children}
    </Suspense>
  </ErrorBoundary>;
}
