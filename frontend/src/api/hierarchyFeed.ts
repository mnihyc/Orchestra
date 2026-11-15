/** Native hierarchy selectors are metadata addresses, never authority tokens. */
import { record } from '@/api/cache';

export interface HierarchyLocator { kind:'hierarchy' }

export function hierarchyQueryKey(query:unknown):string {
  if(!record(query))return '';
  return JSON.stringify(Object.fromEntries(Object.keys(query).filter((key)=>query[key]!==null&&query[key]!==undefined).sort().map((key)=>[key,query[key]])));
}

export function readHierarchyFeed(feed:string):HierarchyLocator|null {
  if(!feed.startsWith('document-tree:'))return null;
  try {
    const coded=feed.slice('document-tree:'.length).replaceAll('-','+').replaceAll('_','/');
    const binary=atob(coded),bytes=Uint8Array.from(binary,(character)=>character.charCodeAt(0));
    const value:unknown=JSON.parse(new TextDecoder().decode(bytes));
    if(!record(value)||value.kind!=='hierarchy')return null;
    return {kind:'hierarchy'};
  } catch {return null;}
}

/** One source follows all open pages of this root, independent of their count. */
export function hierarchyFeed():string {
  const bytes=new TextEncoder().encode(JSON.stringify({kind:'hierarchy'}));
  let binary='';for(const byte of bytes)binary+=String.fromCharCode(byte);
  return `document-tree:${btoa(binary).replaceAll('+','-').replaceAll('/','_').replace(/=+$/u,'')}`;
}
