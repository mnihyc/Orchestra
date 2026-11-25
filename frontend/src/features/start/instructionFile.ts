/** The shared editor and start-conflict editor observe the same native file and directory ledger. */
import { useQueryClient } from '@tanstack/react-query';
import { useEffect,useMemo,useRef } from 'react';
import type { Door } from '@/api/workspace';
import type { Environment } from '@/generated/api-types';
import { invalidateHead,useFileHead } from '@/features/files/content';
import { workspaceBinding } from '@/features/files/door';
import { useDirectoryTree } from '@/features/files/useDirectoryTree';
import { directoryKey } from '@/realtime/stores/environment';

export function useInstructionFile(environment:Environment,path:string) {
  const cache=useQueryClient();
  const door=useMemo<Door>(()=>({environmentId:environment.id,threadId:null}),[environment.id]);
  const binding=workspaceBinding(environment);
  const read=useFileHead(door,path,binding);
  const parent=path.includes('/')?path.slice(0,path.lastIndexOf('/')):'';
  const {directories,capacity,tree}=useDirectoryTree(door,parent,binding);
  const directory=directories.get(directoryKey(environment.id,parent));
  const entry=directory?.entries.find((candidate)=>candidate.path===path);
  const observed=useRef<{binding:string;generation:number;stamp:string}|null>(null);
  useEffect(()=>{
    if(directory===undefined||directory.page===null&&!directory.invalidated)return;
    const stamp=JSON.stringify([entry?.mtime,entry?.size]),before=observed.current;
    if(before?.binding===binding&&before.generation===directory.generation&&before.stamp===stamp)return;
    observed.current={binding,generation:directory.generation,stamp};
    if(before!==null&&(before.binding!==binding||before.generation!==directory.generation)
      ||entry!==undefined&&(entry.mtime!==read.head?.stat.mtime||entry.size!==read.head.stat.size))void invalidateHead(cache,door,path,binding);
  },[binding,directory,entry,read.head,cache,door,path]);
  return {...read,door,binding,directory,capacity,tree};
}
