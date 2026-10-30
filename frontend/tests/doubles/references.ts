/** Reference previews reuse each thing's ordinary reader; a refusal reveals only the known target and request state. */
import type {Chat,Document,Node,ReferencePreview} from '../../src/generated/api-types.ts';
import {Refusal,type Answer,type Person,type World} from './world.ts';

export function previewReference(world:World,person:Person,method:string,path:string,query:URLSearchParams):Answer|null {
 if(method!=='GET'||path!=='/references/preview')return null;
 const id=query.get('id')??'',kind=query.get('kind')??'';
 const base={id,kind,node_kind:null,title:'',place:null,description:'',updated_at:null,updated_by:null,part:null,file:null};
 const doc=kind==='document'?world.documents.docs.get(id):undefined;
 const node=kind==='node'?[...world.trees.values()].flatMap(tree=>[...tree.nodes.values()]).find(row=>row.id===id):undefined;
 const rec=kind==='chat'?world.chats.records.get(id):undefined;
 if(doc===undefined&&node===undefined&&rec===undefined)throw new Refusal(404,'not_found','No such reference');
 const deleted=doc?.deleted===true||node?.deleted===true;
 const hidden=()=>({status:200,body:{...base,unavailable:{deleted,object:deleted||rec?.kind==='direct'?null:{kind:doc===undefined?rec===undefined?node?.kind:'chat':'document',id},permission:rec===undefined?doc===undefined?`${node?.kind}.view`:'doc.view':null,requestable:!deleted&&(doc!==undefined||node!==undefined),pending:null}}});
 if(deleted)return hidden();
 try {
  if(doc!==undefined){
   const shown=world.documents.handle(person,'GET',`/documents/${id}`,new URLSearchParams(),{})?.body as Document;
   let part:ReferencePreview['part']=null;const blockId=query.get('block');
   if(blockId!==null){const blocks=[...doc.blocks.values()].filter(one=>!one.removed);const block=blocks.find(one=>one.id===blockId);if(block!==undefined)part={part:{block:blockId},number:blocks.indexOf(block)+1,anchor:blockId,text:JSON.stringify(block.content),by:block.updated_by};}
   return {status:200,body:{...base,title:shown.title,description:shown.description,place:shown.path.at(-1)??null,updated_at:shown.updated_at,updated_by:shown.updated_by,part}};
  }
  if(node!==undefined){const shown=world.places.handle(person,'GET',`/nodes/${id}`,new URLSearchParams(),{})?.body as Node;return {status:200,body:{...base,title:shown.name,node_kind:shown.kind}};}
  const shown=world.chats.handle(person,'GET',`/chats/${id}`,new URLSearchParams(),{})?.body as Chat;
  return {status:200,body:{...base,title:shown.name??shown.members?.map(user=>user.display_name).join(', ')??'',place:shown.node}};
 }catch(error){if(error instanceof Refusal && [403,404].includes(error.answer.status))return hidden();throw error;}
}
