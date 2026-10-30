/** Inherited passive reads only; imperative requests always carry an explicit read ref. */
import { createContext, createElement, useContext, useMemo, type ReactNode } from 'react';
import { threadReadTarget, type ThreadReadRef, type ThreadReadTarget } from '@/realtime/threadScope';

const ThreadReadContext = createContext<ThreadReadRef | null>(null);
export function ThreadReadProvider({ read, children }: { read: ThreadReadRef | null; children: ReactNode }): ReactNode {
  return createElement(ThreadReadContext.Provider,{ value: read },children);
}
export function useThreadReadRef(target: ThreadReadTarget): ThreadReadRef {
  const inherited = useContext(ThreadReadContext);
  return useMemo(() => typeof target === 'string' && inherited?.threadId === target ? inherited : threadReadTarget(target),[inherited,target]);
}
