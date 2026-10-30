/** Library figures retain their units, hidden-name cuts and unknown coverage on compact rows and pages. */
import { cleanup, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { LibraryCounts } from '@/features/resources/Library';
import type { ResourceFacts } from '@/generated/api-types';
import { show } from './support';

vi.mock('@/features/people/NamedChip',()=>({KnownNodeChip:({node}:{node:{name:string}})=><span>{node.name}</span>}));
const facts:ResourceFacts={subject:{kind:'resource',object:{kind:'skill',id:'skill'}},as_of:'2026-10-27T00:00:00Z',dependents_complete:true,
 dependents:[{relation:'attachment',count:3,hidden:2,until:null,visible:[{object:{kind:'thread',id:'visible'},name:'Visible conversation',relation:'attachment',until:null}]}],
 uses:{kind:'skill_invocation',coverage:{state:'partial',since:'2026-10-01T00:00:00Z'},all_time:120,last_30_days:20,people:4,people_30_days:2,last_used_at:'2026-10-27T00:00:00Z'}};
afterEach(cleanup);

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
});
