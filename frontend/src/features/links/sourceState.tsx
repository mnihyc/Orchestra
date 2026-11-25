/** Expanded mention views belong to the document panel, beyond a filtered row's lifetime. */
import { createContext, useContext, useState, type ReactNode } from 'react';
interface Opening { open: boolean; text: string }
interface SourceState { rows: ReadonlyMap<string, Opening>; change: (id: string, update: (before: Opening) => Opening) => void }
const Context = createContext<SourceState | null>(null);
const empty: Opening = { open: false, text: '' };
export function LinkSourceState({ children }: { children: ReactNode }) {
  const [rows, setRows] = useState<ReadonlyMap<string, Opening>>(new Map());
  return <Context value={{ rows, change: (id, update) => setRows((before) => new Map(before).set(id, update(before.get(id) ?? empty))) }}>{children}</Context>;
}
export function useLinkSourceState(id: string) {
  const context = useContext(Context), [local, setLocal] = useState(empty);
  const value = context?.rows.get(id) ?? local;
  const change = (update: (before: Opening) => Opening) => { if (context === null) setLocal(update); else context.change(id, update); };
  return { ...value, setOpen: (open: boolean) => change((before) => ({ ...before, open })), setText: (text: string) => change((before) => ({ ...before, text })) };
}
