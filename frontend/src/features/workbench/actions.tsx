/** A shown view puts its live actions in its group's tab row or its section's header. */
import { type ReactNode, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

import type { Tab } from './layout';
import { viewKey } from './viewIdentity';

const homes = new Map<string, HTMLElement>();
function homeOf(key: string): HTMLElement {
  let home = homes.get(key);
  if (home === undefined) {
    home = document.createElement('span');
    home.className = 'inline-flex items-center gap-1';
    homes.set(key, home);
  }
  return home;
}

export function forgetViewActions(key: string): void {
  homes.get(key)?.remove();
  homes.delete(key);
}

export function ViewActionSlot({ tab }: { tab: Tab }) {
  const slot = useRef<HTMLSpanElement>(null);
  const key = viewKey(tab);
  useLayoutEffect(() => {
    const parent = slot.current;
    if (parent === null) return;
    const home = homeOf(key);
    parent.appendChild(home);
    return () => { if (home.parentElement === parent) parent.removeChild(home); };
  }, [key]);
  return <span ref={slot} data-view-actions={tab.view} className="inline-flex items-center" />;
}

/** State and mutations belong to the view; moving the action does not mount another reader. */
export function ViewActions({ tab, children }: { tab: Tab; children: ReactNode }) {
  const [home] = useState(() => homeOf(viewKey(tab)));
  return createPortal(children, home);
}
