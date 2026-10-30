/** Catches a dialog's own rendering, including failures before its shared frame mounts. */
import type { ReactNode } from 'react';

import { ContentFailure } from '@/ui/ContentBoundary';
import { Dialog } from '@/ui/Dialog';
import { ErrorBoundary } from '@/ui/ErrorBoundary';

export function DialogBoundary({ title, onClose, children, resetKey }: { title: string; onClose: () => void; children: ReactNode; resetKey?: unknown }) {
  return <ErrorBoundary resetKey={resetKey} fallback={(_error, retry) => <Dialog open title={title} onOpenChange={(open) => { if (!open) onClose(); }}>
    <ContentFailure retry={retry} />
  </Dialog>}>{children}</ErrorBoundary>;
}
