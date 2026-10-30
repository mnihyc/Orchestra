/** A registered program's declared title, reused by the registry and resource pickers. */
import type { MachineProgram } from '@/generated/api-types';

function object(value:unknown):Record<string,unknown> {return typeof value==='object' && value!==null && !Array.isArray(value)?value as Record<string,unknown>:{};}
function text(value:unknown):string|null {return typeof value==='string' && value!==''?value:null;}
function isList(value:unknown):value is readonly unknown[] {return Array.isArray(value);}

export function machineProgramName(entry:Pick<MachineProgram,'descriptor'|'name'>):string {
  const descriptor=object(entry.descriptor);
  return text(descriptor.title) ?? text(object(descriptor.harness).name) ?? entry.name;
}

/** Older connectors report the same declarations under plugins; neither source needs a live read. */
export function reportedAdapterName(facts:Record<string,unknown>,kind:string):string|null {
  for(const source of [object(facts.programs).entries,facts.plugins]) {
    if(!isList(source))continue;
    for(const entry of source) {
      const row=object(entry);
      if(row.kind!=='agent' && row.kind!=='adapter')continue;
      const descriptor=object(row.descriptor);
      if(descriptor.kind!==kind || row.ready===false || row.state==='failed')continue;
      const name=text(row.name);
      if(name!==null)return machineProgramName({descriptor:row.descriptor,name});
    }
  }
  return null;
}
