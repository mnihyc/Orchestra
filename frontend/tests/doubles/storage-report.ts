/** Named retained records cover caches, recovery, private dependents and an away store. */
import type { CleanupChoice, CleanupPlan, Job, KeptRecord, StorageDimension, StorageQuery, StorageReport, StorageTotals } from '@/generated/api-types';
import { me, userRef } from './fixtures';

const id = (value: number) => `00000000-0000-4000-8000-${String(value).padStart(12,'0')}`;
const host = {id:id(601),name:'Build host',address:'acme.ada.build'};
const away = {id:id(602),name:'Offline host',address:'acme.ada.offline'};
const owner = userRef(me(1,'ada','Ada'));
export const STORAGE_CLEANUP_JOB = id(680);

const totals = (rows: readonly KeptRecord[]): StorageTotals => ({items:rows.length,bytes:rows.reduce((sum,row)=>sum+(row.bytes ?? 0),0),unknown:rows.filter((row)=>row.bytes===null).length});

export function storageReport(query: StorageQuery = {}): StorageReport {
  const now=new Date().toISOString(),day=24*60*60*1000;
  const base: KeptRecord = {id:'image',kind:'image',name:'Cached image',resource:{kind:'host',id:host.id},place:null,host,machine:null,owner,bytes:4096,
    accounting_key:'image',included_in:null,why:'cache',since:new Date(Date.now()-8*day).toISOString(),until:new Date(Date.now()-day).toISOString(),facts:null,
    observed_at:now,availability:'ready',can_clean:true,blocker:null};
  const image:KeptRecord={...base,facts:{subject:{kind:'artifact',host_id:host.id,artifact_kind:'image',id:'image'},as_of:now,dependents:[{relation:'serves',count:1,hidden:0,until:null,visible:[{object:{kind:'template',id:id(603)},name:'Rust workspace',relation:'serves',until:null}]}],dependents_complete:true,uses:null}};
  const recovery:KeptRecord={...base,id:'recovery',kind:'environment_files',name:'Workspace recovery',resource:{kind:'environment',id:id(604)},host:null,bytes:null,accounting_key:'recovery',why:'restore',since:new Date(Date.now()-day).toISOString(),until:new Date(Date.now()+7*day).toISOString(),
    facts:{subject:{kind:'resource',object:{kind:'environment',id:id(604)}},as_of:now,dependents:[{relation:'binding',count:3,hidden:2,until:null,visible:[{object:{kind:'thread',id:id(605)},name:'Repair redirects',relation:'binding',until:null}]}],dependents_complete:true,uses:null}};
  const tool:KeptRecord={...base,id:'tool',kind:'tool_version',name:'Connector version',host:away,resource:{kind:'host',id:away.id},bytes:null,accounting_key:'tool',why:'unknown',since:null,until:null,availability:'away',blocker:'node_away',facts:null};
  const rows=[image,recovery,tool].filter((row)=>((query.kind===null || query.kind===undefined) || row.kind===query.kind) && ((query.why===null || query.why===undefined) || row.why===query.why));
  const dimensions:StorageDimension[]=['host','kind','place','owner','age','why'];
  const breakdowns=dimensions.map((by)=>{
    const groups=new Map<string,KeptRecord[]>();
    for(const row of rows) {
      const key=by==='host' ? row.host?.id ?? 'unknown' : by==='owner' ? row.owner?.id ?? 'unknown' : by==='kind' ? row.kind : by==='why' ? row.why : 'unknown';
      groups.set(key,[...(groups.get(key) ?? []),row]);
    }
    return {by,rows:[...groups].map(([key,members])=>({key,name:by==='host' ? members[0]?.host?.name ?? 'unknown' : key,object:by==='host' && key!=='unknown' ? {kind:'host' as const,id:key} : null,owner:by==='owner' ? owner : null,totals:totals(members)}))};
  });
  return {scope:query.scope ?? 'me',name:'',at:now,totals:totals(rows),breakdowns,items:rows,next_cursor:null,unavailable:rows.some((row)=>row.availability==='away') ? [{host:away,reason:'node_away'}] : []};
}

/** The same selected records keep their real cache/data/away choices in a browser preview. */
export function storageCleanupPlan(selected: readonly string[]): CleanupPlan {
  return { id: id(681), expires_at: new Date(Date.now() + 15 * 60_000).toISOString(), confirmation_name: 'My storage', people: [], other_people: 0, notify: false,
    items: storageReport().items.filter((row) => selected.includes(row.id)).map((record) => ({ record,
      choices: record.blocker !== null ? ['keep'] : record.kind === 'image' ? ['keep', 'remove'] : ['keep', 'purge'],
      selected: record.kind === 'image' ? 'remove' : 'keep',
    })),
  };
}

/** A completed durable result names only the requested rows, with no claim that kept rows were erased. */
export function storageCleanupResult(plan: CleanupPlan, choices: ReadonlyMap<string, CleanupChoice>): Job {
  const at = new Date().toISOString();
  return { id: STORAGE_CLEANUP_JOB, kind: 'storage_cleanup', state: 'succeeded', node_id: null, environment_id: null, provider_id: null, batch_id: null,
    created_at: at, created_by: owner, started_at: at, finished_at: at, error: null, log: null,
    progress: { phase: 'cleanup', percent: 100, phases: [{ name: 'cleanup', state: 'done', weight: 100 }], files_done: 0, files_total: null, bytes_done: 0, bytes_total: null, eta_seconds: null, rate_bps: null, message: null },
    result: { cleanup: plan.items.map((item) => ({ id: item.record.id, outcome: item.record.blocker !== null ? 'blocked' : (choices.get(item.record.id) ?? item.selected) === 'keep' ? 'kept' : 'removed', reason: item.record.blocker })) },
  };
}

/** Operator rows carry commands from protected metadata and have no API erase fate. */
export function operatorStoragePlan(): CleanupPlan {
  const at = new Date().toISOString();
  const base = storageReport().items[0];
  if (base === undefined) throw new Error('The storage fixture contains an inventory object.');
  const records: KeptRecord[] = [
    {...base,id:'release:server:old',accounting_key:'release:server:old',kind:'release',name:'server/v0.1.0',resource:null,host:null,owner:null,bytes:8192,why:'cache',until:null,operator_command:"'/srv/Orch files/update.sh' prune"},
    {...base,id:'dump:orch-backup.dump',accounting_key:'dump:orch-backup.dump',kind:'dump',name:'orch-backup.dump',resource:null,host:null,owner:null,bytes:16384,why:'restore',until:null,operator_command:"'/srv/Orch files/update.sh' dumps remove 'orch-backup.dump'"},
  ];
  for (const record of records) record.facts={subject:{kind:'server_artifact',artifact_kind:record.kind,id:record.id},as_of:at,dependents_complete:true,dependents:[],uses:null};
  return {id:id(682),expires_at:new Date(Date.now()+15*60_000).toISOString(),confirmation_name:'Storage',people:[],other_people:0,notify:false,
    items:records.map((record)=>({record,choices:['keep'],selected:'keep'}))};
}

/** The report and preflight share the exact same operator records. */
export function operatorStorageReport(): StorageReport {
  const plan=operatorStoragePlan();
  return {...storageReport(),scope:'system',name:'System storage',items:plan.items.map((item)=>item.record),totals:totals(plan.items.map((item)=>item.record)),breakdowns:[],unavailable:[]};
}
