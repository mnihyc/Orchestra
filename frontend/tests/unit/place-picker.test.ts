/** Native places retain their admitted path, colours and folder meaning without a second tree read. */
import {describe,expect,it} from 'vitest';
import type {NameHit,NodeRef} from '@/generated/api-types';
import {placesOfNames} from '@/features/settings/PlacePicker';
const root:NodeRef={id:'org',kind:'org',name:'Acme',redacted:false};
const web:NodeRef={id:'web',kind:'project',name:'Web',redacted:false};
const hit=(id:string,kind:NameHit['kind'],name:string,path:NodeRef[],more:Partial<NameHit>={}):NameHit=>({id,kind,name,path,home_path:[],address:null,archived:false,avatar:null,color:null,custom_color:null,owner:null,recent:false,tag:null,thread_attention:null,username:null,...more});
describe('the places a picker offers',()=>{
 const rows=[hit('org','org','Acme',[root],{color:3}),hit('web','project','Web',[root,web],{color:5}),hit('folder','project','Folder',[root,web,{id:'folder',kind:'project',name:'Folder',redacted:false}]),hit('login','topic','Fix login',[root,web,{id:'login',kind:'topic',name:'Fix login',redacted:false}]),hit('main','thread','Main thread',[root,web,{id:'main',kind:'thread',name:'Main thread',redacted:false}])];
 it('keeps native page order and full paths while leaving threads out unless asked',()=>{
  expect(placesOfNames(rows,' / ','Hidden').map((place)=>[place.id,place.path])).toEqual([['org','Acme'],['web','Acme / Web'],['folder','Acme / Web / Folder'],['login','Acme / Web / Fix login']]);
 });
 it('opts into threads while keeping non-place kinds out',()=>{
  expect(placesOfNames([...rows,hit('doc','document','Notes',[])],' / ','Hidden',true).map((place)=>place.id)).toEqual(['org','web','folder','login','main']);
 });
 it('carries each native colour, including null for the kind default',()=>{
  expect(placesOfNames(rows,' / ','Hidden').map((place)=>place.color)).toEqual([3,5,null,null]);
 });
 it('recognises a project folder from its actual native parent',()=>{
  expect(placesOfNames(rows,' / ','Hidden').map((place)=>place.folder)).toEqual([false,false,true,false]);
 });
 it('preserves a native custom colour',()=>{
  expect(placesOfNames([hit('web','project','Web',[root,web],{custom_color:'#3a7bd5'})],' / ','Hidden')[0]?.custom_color).toBe('#3a7bd5');
 });
 it('keeps a redacted ancestor unnamed and retains the actual root identity',()=>{
  const [place]=placesOfNames([hit('web','project','Web',[{...root,name:null,redacted:true},web])],' / ','Hidden');
  expect(place?.path).toBe('Hidden / Web');expect(place?.rootOrgId).toBe('org');expect(place?.rootName).toBeUndefined();
 });
});
