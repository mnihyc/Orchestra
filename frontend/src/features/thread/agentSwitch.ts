/** Where a sub-agent's conversation can be shown in place of the main one, the way to do it: the screen of a thread gives it, a panel or a stored view does not. */
import { createContext, useContext } from 'react';

export const AgentSwitchContext = createContext<((step: string) => void) | null>(null);

export function useAgentSwitch(): ((step: string) => void) | null {
  return useContext(AgentSwitchContext);
}
