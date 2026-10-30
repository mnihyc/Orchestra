/** Large retained histories do not turn a paste recovery into a history walk or a quadratic fold build. */
import { expect, it, vi } from 'vitest';
import type { Item, ThreadInput, Turn } from '@/generated/api-types';
import { turnPasteSource } from '@/lib/turnPasteSource';
import { forgetViewChoiceDeviceData,readViewChoice,writeViewChoice } from '@/lib/viewChoices';
import { buildRows } from '@/features/timeline/rows';
import { EMPTY_VIEW } from '@/realtime/stores/thread';
import { agentMessage, otherTool, reasoning, turn, userMessage } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';

class PointMap<T> extends Map<string,T> {
  reads=0;
  override get(key:string):T|undefined {this.reads+=1;return super.get(key);}
}

it('resolves 1000 failed-input references in a million-item world with at most two point reads each',()=>{
  const items=new PointMap<Item>();const base=agentMessage('base','', {turn:'old'});
  for(let i=0;i<1_000_000;i+=1)items.set(`item-${i}`,{...base,id:`item-${i}`,turn_id:`turn-${i%1000}`});
  const turns=new Map<string,Turn>();const inputs=new PointMap<ThreadInput>();
  const author=userRef(me(1,'scale-reader','Scale reader'));
  for(let i=0;i<1000;i+=1){
    const id=`turn-${i}`,inputId=`input-${i}`,itemId=`source-${i}`;
    turns.set(id,turn(id,{ordinal:i+1,state:'failed',inputId,tokens:10_000_000}));
    items.set(itemId,userMessage(itemId,'Original words',{turn:id,inputId,author}));
  }
  expect(items.size).toBe(1_001_000);
  expect([...turns.values()].reduce((sum,row)=>sum+(row.usage?.total_tokens??0),0)).toBe(10_000_000_000);
  const enumerate=()=>{throw new Error('A paste source must not enumerate retained history');};
  items.values=enumerate;items.entries=enumerate;items[Symbol.iterator]=enumerate;
  for(let i=0;i<1000;i+=1)expect(turnPasteSource(inputs,items,`input-${i}`,`source-${i}`)?.[0]).toEqual({type:'text',text:'Original words'});
  expect(items.reads).toBe(1000);expect(inputs.reads).toBe(1000);
},120_000);

it('builds a million work steps with linear item reads and one indexed original-source pointer',()=>{
  const id='large-turn',items=new PointMap<Item>();const order:string[]=[];
  const source=userMessage('original','Question',{turn:id,author:userRef(me(1,'scale-reader','Scale reader'))});items.set(source.id,source);order.push(source.id);
  const base=reasoning('base','Working',{turn:id});
  for(let i=0;i<999_999;i+=1){const key=`step-${i}`;items.set(key,{...base,id:key});order.push(key);}
  const tool=otherTool('last-tool',{name:'Lookup',args:{},turn:id});items.set(tool.id,tool);order.push(tool.id);
  const viewed=turn(id,{ordinal:1,state:'completed',inputId:'original-input',tokens:100_000_000});
  const rows=buildRows({...EMPTY_VIEW,items,order,turns:new Map([[id,viewed]])},{detail:'summary',hideHistory:false,foldOpened:new Set(),meId:null});
  expect(items.size).toBe(1_000_001);
  expect(items.reads).toBeLessThanOrEqual(16*items.size);
  expect(rows.find((row)=>row.kind==='footer')).toMatchObject({kind:'footer',turnId:id,inputItemId:'original'});
  expect(rows.filter((row)=>row.kind==='fold')).toHaveLength(1);
},120_000);

it('bounds unavailable-storage fallback across 100000 visited pages while retaining recent valid choices',()=>{
  const blocked=vi.spyOn(Storage.prototype,'setItem').mockImplementation(()=>{throw new DOMException('Storage unavailable','QuotaExceededError');});
  try {
    const value='x'.repeat(256);
    for(let i=0;i<100_000;i+=1)writeViewChoice('scale-fallback',`page.${i}`,'state',value);
    expect(readViewChoice('scale-fallback','page.99999','state','default')).toBe(value);
    expect(readViewChoice('scale-fallback','page.0','state','default')).toBe('default');
  } finally {blocked.mockRestore();}
},120_000);

it('clears a failed-storage override with device data without clearing another reader’s fallback',()=>{
  writeViewChoice('fallback-clear','page.first','state','stored');
  const storedKey='orch.fallback-clear.choices.page.first.state';
  const blocked=vi.spyOn(Storage.prototype,'setItem').mockImplementation(()=>{throw new DOMException('Storage full','QuotaExceededError');});
  try {
    writeViewChoice('fallback-clear','page.first','state','changed');
    writeViewChoice('other-fallback','page.first','state','other');
    expect(readViewChoice('fallback-clear','page.first','state','default')).toBe('changed');
    localStorage.removeItem(storedKey);
    forgetViewChoiceDeviceData('fallback-clear',[storedKey]);
    expect(readViewChoice('fallback-clear','page.first','state','default')).toBe('default');
    expect(readViewChoice('other-fallback','page.first','state','default')).toBe('other');
    writeViewChoice('fallback-clear','page.second','state','another');
    forgetViewChoiceDeviceData('fallback-clear',null);
    expect(readViewChoice('fallback-clear','page.second','state','default')).toBe('default');
    expect(readViewChoice('other-fallback','page.first','state','default')).toBe('other');
  } finally {blocked.mockRestore();}
});
