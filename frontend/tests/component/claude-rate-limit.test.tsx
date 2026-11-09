/** A native quota notice names its historical provider and escapes the program's actual words. */
import { cleanup, screen } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { MarkerLine } from '@/features/timeline/items/Marker';
import type { Segment } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { marker } from '../doubles/thread-rows';
import { World } from '../doubles/world';
import { show } from './support';

const ID='rate-limit-thread';
const context:TimelineContext={threadId:ID,threadName:'Quota',detail:'steps',recent:new Set(),layout:'one',setLayout:()=>undefined,
  meId:null,canInteract:false,canManage:false,live:true,lit:null,selection:NOT_PICKING};
afterEach(async()=>{cleanup();threadWriter.evict(ID);await setLocale('en');});
for(const locale of ['en','zh-CN'] as const) it(`keeps a native limit notice with the actual segment's provider (${locale})`,async()=>{
  await setLocale(locale);
  const world=new World(),person=world.people.get('ada'),place=world.places.rowOf(ACME.mainThread);
  if(person===undefined||place===undefined)throw new Error('The native fixture needs its thread');
  const native=world.threads.thread(person,place);
  if(native.binding===null)throw new Error('The historical provider needs the native binding');
  const historical:Segment={id:'original',ordinal:1,agent:native.binding.agent,provider:{...native.binding.provider,name:'Original provider'},
    environment:native.environment,arrangement:native.binding.arrangement,credential_path:native.binding.isolation.credential_path,
    executor:null,tool_set:null,env_generation:1,started_by:native.owner,started_at:'2026-11-07T00:00:00Z',ended_at:'2026-11-07T00:01:00Z',
    end_reason:null,import_fidelity:null,import_report:null,seq_updated:1};
  const current:Segment={...historical,id:'later',ordinal:2,provider:{...native.binding.provider,name:'Later provider'}};
  threadWriter.reset(ID,{segments:[historical,current]},false);
  const words='Native warning: <script>escaped content</script>';
  const item=marker('native-limit',{marker:'harness_changed',data:{program:'Claude Code',change:'rate_limit',subject:null,
    previous:{state:'unsupported'},value:{status:'allowed_warning',message:words},expected:{state:'unsupported'},changed:false,diverged:false,
    next_turn:false,turn_id:null,reported_at:'2026-11-07T00:00:00Z'}});
  if(item.kind!=='marker'||item.payload.marker!=='harness_changed')throw new Error('A rate notice is a harness marker');
  const actual={...item,segment_id:'original',payload:item.payload};
  const view=show(<TimelineContextValue.Provider value={context}><MarkerLine item={actual}/></TimelineContextValue.Provider>);
  expect(document.body).toHaveTextContent(i18n.t('harnessChanges.rateLimit',{provider:'Original provider',message:words}));
  expect(document.body).not.toHaveTextContent('Later provider');
  expect(document.body.querySelector('script')).toBeNull();
  const unknown={...actual,segment_id:'absent',payload:{...actual.payload,data:{...actual.payload.data,value:{status:'future-native-status'}}}};
  view.rerender(<TimelineContextValue.Provider value={context}><MarkerLine item={unknown}/></TimelineContextValue.Provider>);
  expect(document.body).toHaveTextContent(i18n.t('harnessChanges.rateLimit',{provider:i18n.t('harnessChanges.provider'),message:'future-native-status'}));
  expect(screen.queryByText('Original provider')).toBeNull();
});
