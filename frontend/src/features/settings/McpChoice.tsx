/** The existing MCP enablement map, named and grouped without sending an action while browsing. */
import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { X } from 'lucide-react';
import { mcpChoicesQuery } from '@/api/resource-tools';
import { ResourceChip, resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { holds } from '@/lib/permissions';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { ResourcePicker } from '@/ui/ResourcePicker';
import { Switch } from '@/ui/Switch';
import type { ValueControlProps } from './ValueControl';

const switches=(value:unknown):[string,boolean][]=>value!==null&&typeof value==='object'&&!Array.isArray(value)
  ?Object.entries(value).flatMap(([id,on])=>typeof on==='boolean'?[[id,on] as [string,boolean]]:[]):[];
export function McpSettingChoice({value,effective,environment,label,onChange,onEdited,disabled=false,error}:ValueControlProps&{environment:string|null;effective:unknown}) {
  const {t}=useTranslation();const[open,setOpen]=useState(false);
  const chosen=switches(value);
  const catalogue=useQuery({...mcpChoicesQuery(environment),enabled:open||chosen.length>0});
  const rows=catalogue.data??[];const inherited=new Map(switches(effective));
  const write=(next:[string,boolean][])=>{onEdited?.();onChange(Object.fromEntries(next));};
  return <div className="flex min-w-0 flex-col gap-2">
    {chosen.length>0&&<ul aria-label={label} className="flex flex-col gap-2">{chosen.map(([id,on])=>{
      const server=rows.find((row)=>row.id===id);const name=server?.name??t(catalogue.isPending?'common.loading':'settings.resourceChoice.unavailable');
      return <li key={id} className="flex min-w-0 flex-wrap items-center gap-2"><span className="min-w-0 flex-1">{server===undefined?name:<ResourceChip subject={{kind:'mcp_server',server}} hint={false}/>}</span>
        <Switch label={t('resourceGroups.mcpEnabled',{name})} checked={on} disabled={disabled||(!on&&!holds(server?.access,'mcp.use'))}
          onCheckedChange={(next)=>write(chosen.map(([key,before])=>[key,key===id?next:before]))}/>
        <IconButton size="sm" icon={<X aria-hidden/>} label={t('resourceGroups.mcpRemove',{name})} disabled={disabled}
          onClick={()=>write(chosen.filter(([key])=>key!==id))}/></li>;
    })}</ul>}
    <ResourcePicker pickerKey="mcp-enabled" label={t('resourceGroups.mcpAdd')} hideLabel value="" open={open} onOpenChange={setOpen}
      disabled={disabled} error={error} loading={catalogue.isFetching} placeholder={t(open&&catalogue.isPending?'common.loading':'resourceGroups.mcpAdd')}
      options={rows.filter((row)=>!chosen.some(([id])=>id===row.id)).map((server)=>({value:server.id,name:server.name,
        resource:{kind:'mcp_server',id:server.id,data:server},...resourceChoice({kind:'mcp_server',server})}))}
      onValueChange={(id)=>{const server=rows.find((row)=>row.id===id);write([...chosen,[id,inherited.get(id)===true&&holds(server?.access,'mcp.use')]]);}}/>
    {catalogue.isError&&<ErrorState compact error={catalogue.error} retrying={catalogue.isRefetching} onRetry={()=>void catalogue.refetch()}/>}
  </div>;
}
