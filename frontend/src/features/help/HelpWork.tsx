/** A reading overlay pauses the work page while keeping its drafts in place. */
import { Activity, type ReactNode } from 'react';
import { useHelpState } from '@/features/help/state';

export function HelpWork({ children }: { children: ReactNode }) {
  const reading = useHelpState((state) => state.overlay);
  return <Activity mode={reading ? 'hidden' : 'visible'}><div className="contents" data-help-work>{children}</div></Activity>;
}
