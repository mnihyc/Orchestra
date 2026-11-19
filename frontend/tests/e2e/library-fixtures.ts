/** The same resource records drive the library walk and its reference-frame request measurement. */
import type { EverythingResource, ResourceFacts, ResourceUseKind, Skill } from '../../src/generated/api-types';
import { detailAccess, detailAgent, detailEnvironment, detailHome, detailHost, detailMachine, detailMcp, detailMount, detailTemplate, detailUnitProvider } from '../component/resource-detail-fixtures';

const skill:Skill={id:'library-skill',name:'lookup',description:'Look up project facts',address:'workshop.lookup',home:detailHome,org_id:'team',owner:detailAgent.owner,enabled:true,latest:null,versions:0,draft:null,origin:null,revision:1,access:{...detailAccess,permissions:[...detailAccess.permissions,'skill.view','skill.use','skill.manage']}};
export const resources:Record<string,EverythingResource>={providers:{kind:'provider',value:detailUnitProvider},agents:{kind:'agent',value:detailAgent},environments:{kind:'environment',value:detailEnvironment},hosts:{kind:'host',value:detailHost},templates:{kind:'template',value:detailTemplate},mcp:{kind:'mcp_server',value:detailMcp},mounts:{kind:'mount',value:detailMount},machines:{kind:'machine',value:detailMachine},skills:{kind:'skill',value:skill}};
export function facts(resource:EverythingResource):ResourceFacts {
 const kind=resource.kind;
 const unit:ResourceUseKind|null=kind==='mount'||kind==='machine'?null:kind==='mcp_server'?'mcp_call':kind==='skill'?'skill_invocation':kind==='host'||kind==='template'?'environment_made':'turn';
 return {subject:{kind:'resource',object:{kind,id:resource.value.id}},as_of:'2026-10-27T00:00:00Z',dependents_complete:true,
  dependents:[{relation:['provider','agent','environment'].includes(kind)?'binding':'attachment',count:3,hidden:2,until:null,visible:[{object:null,name:'Visible destination',relation:'attachment',until:null}]}],
  uses:unit===null?null:{kind:unit,coverage:{state:'partial',since:'2026-10-01T00:00:00Z'},all_time:120,last_30_days:12,people:2,people_30_days:1,last_used_at:'2026-10-27T00:00:00Z'}};
}

