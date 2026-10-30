/** A mounted view owns its DOM, local resource context and metadata independently of the URL. */
import { createContext, useContext, type RefObject } from 'react';
import type { InspectorTarget } from './layout';

export interface ViewInstance {
  id: string;
  root: RefObject<HTMLDivElement | null>;
  target: InspectorTarget | null;
  headingOwner: string | null;
  content: boolean;
  focusedContent: boolean;
}
export const ViewInstanceContext = createContext<ViewInstance | null>(null);
export function useViewInstance(): ViewInstance | null { return useContext(ViewInstanceContext); }
export function useContentFocused(): boolean { return useViewInstance()?.focusedContent ?? true; }
