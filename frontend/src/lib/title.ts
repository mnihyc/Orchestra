/** The title of the browser's tab: the screen in view, then the name of the system. */
import { useEffect } from 'react';

/** What the tab reads when nothing else is known. */
const PRODUCT = 'Orch';

let system = PRODUCT;

/** Tells the title what this installation is called. */
export function setSystemName(name: string): void {
  system = name.trim() === '' ? PRODUCT : name;
}

/** Names the screen in view in the browser's tab, for as long as it is in view. */
export function usePageTitle(title: string | null | undefined, enabled = true): void {
  useEffect(() => {
    if (!enabled) return;
    document.title = title === null || title === undefined || title === '' ? system : `${title} · ${system}`;
  }, [title, enabled]);
}
