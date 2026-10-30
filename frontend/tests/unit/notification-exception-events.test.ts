/** Acknowledged own choices need no echo read; another device updates the active exception page. */
import { afterEach,expect,it } from 'vitest';
import { QueryClient } from '@tanstack/react-query';
import { applyChanges,findAct,userEventChanges } from '@/api/acts';
import { queryClient } from '@/api/query';
import { memberAccess } from '../doubles/fixtures';

const key=['subscription-choice','exception-topic'];
afterEach(()=>queryClient.removeQueries({queryKey:key,exact:true}));
it('does not schedule another read for the echoed acknowledgement, but does for another value',()=>{
  queryClient.setQueryData(key,'off');
  expect(userEventChanges('subscription',{node_id:'exception-topic',level:'off'})).toEqual([]);
  expect(userEventChanges('subscription',{node_id:'exception-topic',level:'all'})).toContainEqual({missing:[{key:['notification-exceptions']}]});
  expect(findAct('PUT','/me/notification-exceptions/exception-topic')?.changes).toEqual([]);
});

it('applies the actual native page boundary and Access before the write resolves, without a repair read',async()=>{
  const cache=new QueryClient();
  cache.setQueryData(['node','exception-topic'],{id:'exception-topic',kind:'topic',name:'Login',access:memberAccess});
  const first={items:[],next_cursor:'later'};
  cache.setQueryData(['notification-exceptions',null],first);
  const page={items:[],next_cursor:null};
  const access={...memberAccess,subscription:'off',effective_subscription:'off',inherited_subscription:'important'};
  const change=findAct('PUT','/me/notification-exceptions/exception-topic',{answer:{access,exceptions:page,page_cursor:'later'}});
  expect(change).not.toBeNull();
  expect(change?.changes.some((row)=>'missing' in row || 'feed' in row || 'key' in row)).toBe(false);
  await applyChanges(change?.changes ?? [],cache);
  expect(cache.getQueryData(['notification-exceptions','later'])).toEqual(page);
  expect(cache.getQueryData(['notification-exceptions',null])).toEqual(first);
  expect(cache.getQueryData(['node','exception-topic'])).toEqual({id:'exception-topic',kind:'topic',name:'Login',access});
  expect(cache.getQueryData(['subscription-choice','exception-topic'])).toBe('off');
});
