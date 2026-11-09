/** Native compound-picker metadata stays with that response, not another operation's cached detail. */
import { createContext, type ReactNode, useContext, useMemo } from 'react';
import type { BindingOptions, Place } from '@/generated/api-types';
import { resourceGroupMeta, type ResourceGroupMeta } from './groups';

export const resourceGroupKey = (kind: string, id: string) => `${kind}:${id}`;
export interface PickerGroups { entries: ReadonlyMap<string, ResourceGroupMeta>; currentHomes: readonly string[]; strict: boolean; screen: string }
const Context = createContext<PickerGroups>({entries:new Map(),currentHomes:[],strict:false,screen:'resources'});
export const useResourcePickerGroups = () => useContext(Context);

/** Compound metadata is already selected by its native producer. Never walk arbitrary response UUIDs. */
export function compoundPickerGroups(data: unknown): ReadonlyMap<string, ResourceGroupMeta> {
  const rows: readonly unknown[] = data !== null && typeof data === 'object' && 'grouping' in data && Array.isArray(data.grouping) ? data.grouping : [];
  const result=new Map<string,ResourceGroupMeta>();
  for(const row of rows) if(row !== null && typeof row === 'object' && 'kind' in row && 'id' in row && typeof row.kind === 'string' && typeof row.id === 'string')
    result.set(resourceGroupKey(row.kind,row.id),resourceGroupMeta(row));
  return result;
}

export function bindingPickerGroups(data: BindingOptions | undefined): ReadonlyMap<string,ResourceGroupMeta> {
  const result=new Map<string,ResourceGroupMeta>();
  for(const row of data?.providers ?? []) {
    result.set(resourceGroupKey('provider',row.provider.id),resourceGroupMeta(row));
    for(const agent of row.agents) result.set(resourceGroupKey('agent',agent.agent.id),resourceGroupMeta(agent));
  }
  for(const row of data?.environments ?? []) result.set(resourceGroupKey('environment',row.environment.id),resourceGroupMeta(row));
  for(const row of data?.hosts ?? []) result.set(resourceGroupKey('host',row.host.id),resourceGroupMeta(row));
  for(const row of data?.machines ?? []) result.set(resourceGroupKey('machine',row.machine.id),resourceGroupMeta(row));
  return result;
}

export function ResourcePickerScope({entries,currentHomes=[],screen,children}: {
  entries:ReadonlyMap<string,ResourceGroupMeta>; currentHomes?:readonly string[];screen:string;children:ReactNode;
}) {
  const value=useMemo(()=>({entries,currentHomes,strict:true,screen}),[entries,currentHomes,screen]);
  return <Context value={value}>{children}</Context>;
}

export function BindingPickerScope({options,screen,currentHomes,children}:{options:BindingOptions|null|undefined;screen:string;currentHomes?:readonly string[];children:ReactNode}) {
  const entries=useMemo(()=>bindingPickerGroups(options??undefined),[options]);
  return <ResourcePickerScope entries={entries} screen={screen} currentHomes={currentHomes}>{children}</ResourcePickerScope>;
}
export function CompoundPickerScope({data,screen,currentHomes,children}:{data:unknown;screen:string;currentHomes?:readonly string[];children:ReactNode}) {
  const entries=useMemo(()=>compoundPickerGroups(data),[data]);
  return <ResourcePickerScope entries={entries} screen={screen} currentHomes={currentHomes}>{children}</ResourcePickerScope>;
}
export function PlacePickerScope({places,screen,currentHomes,children}:{places:readonly Place[];screen:string;currentHomes?:readonly string[];children:ReactNode}) {
  const entries=useMemo(()=>new Map(places.map((row)=>['environment' in row?resourceGroupKey('environment',row.environment.id):resourceGroupKey('host',row.host.id),resourceGroupMeta(row)])),[places]);
  return <ResourcePickerScope entries={entries} screen={screen} currentHomes={currentHomes}>{children}</ResourcePickerScope>;
}
