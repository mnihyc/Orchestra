/** A heading that opens what is under it, with the app's shared chevron and focus treatment. */
import { ChevronRight } from 'lucide-react';
import type { ReactNode } from 'react';

export function Disclosure({ title, lead, children }: { title: string; lead?: string; children: ReactNode }) {
  return (
    <details className="group min-w-0 rounded-lg border border-border">
      <summary className="focus-inset flex cursor-default list-none items-center gap-2 rounded-lg px-3 py-2 text-sm font-medium text-text select-none hover:bg-bg-subtle [&::-webkit-details-marker]:hidden">
        <ChevronRight className="shrink-0 text-text-subtle transition-transform group-open:rotate-90" size={16} aria-hidden />
        <span className="min-w-0 wrap-anywhere">{title}</span>
        {lead !== undefined && <span className="font-normal text-text-muted">{lead}</span>}
      </summary>
      <div className="flex min-w-0 flex-col divide-y divide-border border-t border-border px-3">{children}</div>
    </details>
  );
}
