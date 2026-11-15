/** A prospective editor reads one source file; it never prepares or walks a workspace. */
import { queryOptions } from '@tanstack/react-query';
import { api,ApiError,ok } from '@/api/client';
import { nodeQuery } from '@/api/nodes';
import { environmentQuery,readFileBytes } from '@/api/workspace';
import { headQuery } from '@/features/files/content';
import { workspaceBinding } from '@/features/files/door';
import { PREVIEW_BYTES } from '@/features/files/viewers';
import { includesInstructions } from './instructionSelection';
import type { NewEnvironmentFiles } from '@/generated/api-types';

const NAME='AGENTS.md';

export const instructionSourceQuery=(files:NewEnvironmentFiles|null|undefined,reader:string)=>queryOptions({
  queryKey:['instruction-file-source',reader,files] as const,retry:false,gcTime:0,
  queryFn:async({client,signal}):Promise<{text:string;hash:string}|null>=>{
    if(files===null||files===undefined)return null;
    if('selection' in files&&!includesInstructions(files.selection))return null;
    if('snapshot_id' in files) {
      const result=await api.GET('/snapshots/{id}/files/content',{params:{path:{id:files.snapshot_id},query:{path:NAME}},headers:{Range:`bytes=0-${PREVIEW_BYTES-1}`},parseAs:'arrayBuffer',signal});
      const hash=/^"([0-9a-f]{64})"$/u.exec(result.response.headers.get('ETag')??'')?.[1];
      if(result.response.status===416&&result.response.headers.get('Content-Range')==='bytes */0'&&hash!==undefined)return {text:'',hash};
      let data:ArrayBuffer;
      try { data=await ok(Promise.resolve(result)); }catch(error) {
        if(error instanceof ApiError&&error.is('not_found')&&error.problem.reason==='not_found')return null;
        throw error;
      }
      const bytes=new Uint8Array(data);
      const total=Number(/\/(\d+)\s*$/u.exec(result.response.headers.get('Content-Range')??'')?.[1]??bytes.length);
      if(hash===undefined||total>bytes.length)throw new Error('The complete source file is not available');
      return {text:new TextDecoder('utf-8',{fatal:true}).decode(bytes),hash};
    }
    let environment:string,path=NAME;
    if('from_environment' in files)environment=files.from_environment;
    else {
      const source=await client.query({...nodeQuery(files.from_thread),staleTime:0});
      if(source.kind!=='thread'||source.environment===null)throw new Error('The source thread has no workspace');
      environment=source.environment.id;
      if(source.directory!=='')path=`${source.directory.replace(/\/+$/u,'')}/${NAME}`;
    }
    const env=await client.query({...environmentQuery(environment),staleTime:0});
    const door={environmentId:environment,threadId:null};
    try {
      const head=await client.query({...headQuery(door,path,workspaceBinding(env)),staleTime:0});
      if(head.brought.state!=='ready'||!head.stat.is_text)throw new Error('The source file is not available as text');
      const value=await readFileBytes(door,path,head.brought.hash,PREVIEW_BYTES,signal);
      if(value.total>value.bytes.length)throw new Error('The complete source file is not available');
      return {text:new TextDecoder('utf-8',{fatal:true}).decode(value.bytes),hash:head.brought.hash};
    }catch(error) {
      if(error instanceof ApiError&&error.is('not_found')&&error.problem.reason==='not_found')return null;
      throw error;
    }
  },
});
