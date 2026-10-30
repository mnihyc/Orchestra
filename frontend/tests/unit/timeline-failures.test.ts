/** Different unreachable tool-environment diagnostics stay distinct in the timeline. */
import { expect,it } from 'vitest';
import { failureKey,sameFailure } from '@/features/timeline/failures';

it('keeps distinct tool-environment failures separate while grouping a repeat',()=>{
  const first={code:'environment_unreachable',message:'executor could not connect'};
  const second={code:'environment_unreachable',message:'executor closed the connection'};
  expect(failureKey(first)).not.toBe(failureKey(second));
  expect(sameFailure(first,second)).toBe(false);
  expect(failureKey(first)).toBe(failureKey({...first}));
  expect(sameFailure(first,{...first})).toBe(true);
});
