/** A mounted view owns its DOM, local resource context and metadata independently of the URL. */
import { createContext, useContext, useMemo, type ReactNode, type RefObject } from 'react';
import type { InspectorTarget } from './layout';

export interface ViewInstance {
  id: string;
  root: RefObject<HTMLDivElement | null>;
  target: InspectorTarget | null;
  headingOwner: string | null;
  content: boolean;
}
const ViewInstanceContext = createContext<ViewInstance | null>(null);
const FocusedContent = createContext(true);

export function ViewInstanceProvider({ value, children }: { value: ViewInstance & { focusedContent: boolean }; children: ReactNode }) {
  const { id, root, target, headingOwner, content, focusedContent } = value;
  const identity = useMemo(() => ({ id, root, target, headingOwner, content }), [id, root, target, headingOwner, content]);
  return <ViewInstanceContext.Provider value={identity}><FocusedContent.Provider value={focusedContent}>
    {children}
  </FocusedContent.Provider></ViewInstanceContext.Provider>;
}
export function useViewInstance(): ViewInstance | null { return useContext(ViewInstanceContext); }
export function useContentFocused(): boolean { return useContext(FocusedContent); }
