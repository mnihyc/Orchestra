/** Explicit usage addresses beat device choices, which remain separate for each chart and each door. */
import { useState } from 'react';
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach,beforeEach,expect,it } from 'vitest';
import { client } from '@/api/client';
import { useUsageChoices,type SearchPatch } from '@/features/usage/choices';
import { patchSearch,readSearch,type UsageSearch } from '@/features/usage/search';
import { show } from './support';

let previousUser:string | null=null;
beforeEach(()=>{previousUser=client.session.getState().userId;client.session.setState({userId:'usage-choice-reader'});});
afterEach(()=>client.session.setState({userId:previousUser}));

function Choices({kind='usage',initial={}}:{kind?:'usage' | 'providerUsage';initial?:UsageSearch}) {
  const [address,setAddress]=useState(initial);
  const [visible,keep]=useUsageChoices(address,kind,'project');
  const change=(patch:SearchPatch)=>{keep(patch);setAddress((before)=>patchSearch(before,patch));};
  return <><output aria-label="Visible choices">{JSON.stringify(visible)}</output><output aria-label="Address">{JSON.stringify(address)}</output>
    <button onClick={()=>change({view:'usage'})}>Usage</button><button onClick={()=>change({view:'performance'})}>Performance</button><button onClick={()=>change({view:'requests'})}>Requests</button>
    <button onClick={()=>change({show:'tokens',series:'token_kind',bucket:'day'})}>Token chart</button>
    <button onClick={()=>change({show:'first_token',series:'provider',bucket:'hour'})}>Performance chart</button>
    <button onClick={()=>change({plan:'history',plan_bucket:'hour'})}>Plan history</button>
    <button onClick={()=>change({by:'person',range:'last_week'})}>Table and period</button>
    <button onClick={()=>change({person:'00000000-0000-4000-8000-000000000001',from:'2026-10-01',to:'2026-10-02'})}>Custom and filter</button>
  </>;
}
const visible=():UsageSearch=>JSON.parse(screen.getByLabelText('Visible choices').textContent) as UsageSearch;

it('restores every chart choice after refresh and a plain visit while preserving the other chart',async()=>{
  const user=userEvent.setup();
  let rendered=show(<Choices />);
  await user.click(screen.getByRole('button',{name:'Token chart'}));
  await user.click(screen.getByRole('button',{name:'Plan history'}));
  await user.click(screen.getByRole('button',{name:'Table and period'}));
  await user.click(screen.getByRole('button',{name:'Performance'}));
  expect(visible()).toMatchObject({show:'speed',series:'model',bucket:'auto'});
  await user.click(screen.getByRole('button',{name:'Performance chart'}));
  const address=readSearch(JSON.parse(screen.getByLabelText('Address').textContent) as Record<string,unknown>);
  rendered.unmount();rendered=show(<Choices initial={address} />);
  expect(visible()).toMatchObject({view:'performance',show:'first_token',series:'provider',bucket:'hour',plan:'history',plan_bucket:'hour',by:'person',range:'last_week'});
  await user.click(screen.getByRole('button',{name:'Usage'}));
  expect(visible()).toMatchObject({view:'usage',show:'tokens',series:'token_kind',bucket:'day'});
  await user.click(screen.getByRole('button',{name:'Requests'}));
  rendered.unmount();rendered=show(<Choices />);
  expect(visible()).toMatchObject({view:'requests',plan:'history',plan_bucket:'hour',by:'person',range:'last_week'});
  rendered.unmount();show(<Choices kind="providerUsage" />);
  expect(visible()).toMatchObject({view:'usage',show:'credits',series:'model',bucket:'auto',plan:'current',plan_bucket:'auto',by:'project',range:'30d'});
});

it('honours explicit default choices in an address and never restores filters or custom dates',async()=>{
  const user=userEvent.setup();
  let rendered=show(<Choices kind="providerUsage" />);
  await user.click(screen.getByRole('button',{name:'Token chart'}));
  await user.click(screen.getByRole('button',{name:'Table and period'}));
  await user.click(screen.getByRole('button',{name:'Custom and filter'}));
  expect(visible()).toMatchObject({from:'2026-10-01',to:'2026-10-02',person:'00000000-0000-4000-8000-000000000001'});
  rendered.unmount();rendered=show(<Choices kind="providerUsage" />);
  expect(visible()).toMatchObject({show:'tokens',range:'last_week'});
  expect(visible().from).toBeUndefined();expect(visible().person).toBeUndefined();
  rendered.unmount();show(<Choices kind="providerUsage" initial={readSearch({view:'usage',show:'credits',series:'model',bucket:'auto',range:'30d'})} />);
  expect(visible()).toMatchObject({view:'usage',show:'credits',series:'model',bucket:'auto',range:'30d'});
});
