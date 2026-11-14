/** Library figures retain their units, hidden-name cuts and unknown coverage on compact rows and pages. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { LibraryCounts } from '@/features/resources/Library';
import type { ResourceFacts } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

vi.mock('@/features/people/NamedChip',()=>({KnownNodeChip:({node}:{node:{name:string}})=><span>{node.name}</span>}));
const facts:ResourceFacts={subject:{kind:'resource',object:{kind:'skill',id:'skill'}},as_of:'2026-10-27T00:00:00Z',dependents_complete:true,
 dependents:[{relation:'attachment',count:3,hidden:2,until:null,visible:[{object:{kind:'thread',id:'visible'},name:'Visible conversation',relation:'attachment',until:null}]}],
 uses:{kind:'skill_invocation',coverage:{state:'partial',since:'2026-10-01T00:00:00Z'},all_time:120,last_30_days:20,people:4,people_30_days:2,last_used_at:'2026-10-27T00:00:00Z'}};
afterEach(async()=>{cleanup();await setLocale('en');});

it('opens a Given figure with only the projected names and the hidden count',async()=>{
 const user=userEvent.setup();show(<LibraryCounts facts={facts}/>);
 expect(screen.getByText('Used 120 invocations')).toBeInTheDocument();
 await user.click(screen.getByText('Given 3 threads'));
 expect(await screen.findByText('Visible conversation')).toBeInTheDocument();
 expect(screen.getByText('and 2 more')).toBeInTheDocument();
});
it('shows unrecorded use as a dash and omits a figure where no use unit exists',()=>{
 const view=show(<LibraryCounts facts={{...facts,dependents_complete:false,uses:{...facts.uses!,coverage:{state:'unrecorded'},all_time:null,last_30_days:null,people:null,people_30_days:null,last_used_at:null}}} full/>);
 expect(screen.getByText('Used – invocations')).toBeInTheDocument();expect(screen.getByText('Not recorded')).toBeInTheDocument();
 view.unmount();
 show(<LibraryCounts facts={{...facts,subject:{kind:'resource',object:{kind:'mount',id:'mount'}},uses:null}}/>);
 expect(screen.queryByText(/Used/)).not.toBeInTheDocument();
});

for(const locale of ['en','zh-CN'] as const) it('names each lifetime and recent use unit for zero, one, plural and unknown counts in '+locale,async()=>{
 await setLocale(locale);
 if(facts.uses===null)throw new Error('The use fixture must report its native kind');
 const view=show(<LibraryCounts facts={facts} full given={false}/>);
 for(const [kind,resource,singular,plural,chinese] of [
  ['turn','provider','turn','turns','轮'],
  ['mcp_call','mcp_server','call','calls','次调用'],
  ['skill_invocation','skill','invocation','invocations','次技能调用'],
  ['environment_made','template','environment','environments','环境'],
  ['provider_made','template','provider','providers','提供方'],
 ] as const)for(const count of [0,1,2,null]){
  const data:ResourceFacts={...facts,subject:{kind:'resource',object:{kind:resource,id:resource}},uses:{...facts.uses,kind,all_time:count,last_30_days:count}};
  view.rerender(<LibraryCounts facts={data} full given={false}/>);
  const number=count===null?'–':String(count);
  const unit=locale==='en'?(count===1?singular:plural):chinese;
  expect(screen.getByText(locale==='en'?`${number} ${unit} all time`:`累计 ${number} ${unit}`,{exact:true})).toBeVisible();
  expect(screen.getByText(locale==='en'?`${number} ${unit} in the last 30 days`:`最近 30 天 ${number} ${unit}`,{exact:true})).toBeVisible();
 }
});

it('keeps only 50 Given names in the DOM across a 100000-name result and can reach later pages',async()=>{
 const visible=Array.from({length:100_000},(_,index)=>({object:null,name:`Visible place ${index+1}`,relation:'attachment' as const,until:null}));
 const user=userEvent.setup();
 show(<LibraryCounts facts={{...facts,subject:{kind:'resource',object:{kind:'mount',id:'mount'}},uses:null,
   dependents:[{relation:'attachment',count:visible.length,hidden:0,until:null,visible}]}}/>);
 await user.click(screen.getByRole('button',{name:/^Given .* environments$/}));
 expect(await screen.findByText('Visible place 1')).toBeVisible();
 expect(screen.getAllByRole('listitem')).toHaveLength(50);
 expect(screen.queryByText('Visible place 51')).not.toBeInTheDocument();
 await user.click(screen.getByRole('button',{name:'Next page'}));
 expect(await screen.findByText('Visible place 51')).toBeVisible();
 expect(screen.getAllByRole('listitem')).toHaveLength(50);
 expect(screen.queryByText('Visible place 1')).not.toBeInTheDocument();
 await user.click(screen.getByRole('button',{name:'Back to beginning'}));
 expect(await screen.findByText('Visible place 1')).toBeVisible();
 expect(screen.getAllByRole('listitem')).toHaveLength(50);
 const search=screen.getByRole('searchbox',{name:i18n.t('library.search')});
 await user.type(search,'100000');
 expect(await screen.findByText('Visible place 100000')).toBeVisible();
 expect(screen.getAllByRole('listitem')).toHaveLength(1);
 await user.clear(search);
 expect(await screen.findByText('Visible place 1')).toBeVisible();
});
