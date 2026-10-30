/** A broad provider invalidation never duplicates the plan read for the same observation clock. */
import { QueryClient } from '@tanstack/react-query';
import { expect,it,vi } from 'vitest';
import { planUsageQuery } from '@/api/planUsage';

const reads=vi.hoisted(()=>vi.fn(()=>Promise.resolve({data:{items:[],next_cursor:null}})));
vi.mock('@/api/client',()=>({api:{GET:reads},ok:async(response:Promise<{data:{items:[];next_cursor:null}}>)=>(await response).data}));

it('reuses a measured snapshot through broad invalidation and reads once for the next facts timestamp',async()=>{
  const client=new QueryClient({defaultOptions:{queries:{retry:false}}});
  const before=planUsageQuery('provider','2026-10-01','2026-10-28','day','2026-10-28T12:00:00Z');
  await client.query(before);
  expect(reads).toHaveBeenCalledTimes(1);
  await client.invalidateQueries({queryKey:['provider','provider']});
  await client.query(before);
  expect(reads).toHaveBeenCalledTimes(1);
  await client.query(planUsageQuery('provider','2026-10-01','2026-10-28','day','2026-10-28T12:05:00Z'));
  expect(reads).toHaveBeenCalledTimes(2);
  client.clear();
});
