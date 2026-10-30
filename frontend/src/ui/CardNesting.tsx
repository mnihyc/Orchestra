/** A hovered card stays present while a person's card opened beside one of its rows is being read. */
import { createContext, type ReactNode, useCallback, useContext, useEffect, useId, useState } from 'react';
const KeepParent = createContext<(()=>void)|null>(null);
export function useKeepParentCard(){return useContext(KeepParent);}
const ParentCard = createContext<((id: string, open: boolean) => void) | null>(null);
export function useChildCard(open: boolean) {
  const parent = useContext(ParentCard);const id = useId();
  const announce=useCallback((next:boolean)=>parent?.(id,next),[parent,id]);
  useEffect(() => { announce(open);return () => announce(false); }, [announce,open]);
  return announce;
}
export function useCardChildren(keep?:()=>void) {
  const [children,setChildren]=useState<ReadonlySet<string>>(()=>new Set());
  const change=useCallback((id:string,open:boolean)=>setChildren((before)=>{if(before.has(id)===open)return before;const next=new Set(before);if(open)next.add(id);else next.delete(id);return next;}),[]);
  return { open:children.size>0, wrap:(content:ReactNode)=><ParentCard.Provider value={change}><KeepParent.Provider value={keep??null}>{content}</KeepParent.Provider></ParentCard.Provider> };
}
