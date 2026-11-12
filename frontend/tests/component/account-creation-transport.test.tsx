/** The actual auth API accepts a completed receipt without adopting it as a session. */
import { afterEach, expect, it, vi } from 'vitest';
import { register, setUp } from '@/api/auth';
import { client } from '@/api/client';
import { newActKey } from '@/lib/requests';

afterEach(()=>{vi.restoreAllMocks();vi.unstubAllGlobals();});
it('retains supplied UUIDs across public retries and adopts no session or secret',async()=>{
  const seen:{path:string;key:string|null;body:string}[]=[];
  vi.stubGlobal('fetch',async(request:Request)=>{
    seen.push({path:new URL(request.url).pathname,key:request.headers.get('Idempotency-Key'),body:await request.text()});
    return Response.json({projection:'receipt',user_id:'actual-created-account',sign_in_required:true},
      {status:request.url.endsWith('/setup')?200:201});
  });
  const adopt=vi.spyOn(client,'adopt');
  const original=client.session.getState();
  const input={username:'created-once',display_name:'Created once',password:'a-valid-secret-1234'};
  const setupKey=newActKey();const registerKey=newActKey();
  for(let attempt=0;attempt<2;attempt++) {
    expect(await setUp(input,setupKey)).toEqual({signedIn:false,alreadyCreated:true});
    expect(await register({...input,accept_invitation:false},registerKey)).toEqual({signedIn:false,alreadyCreated:true});
  }
  expect(seen).toHaveLength(4);
  expect(seen[0]?.key).toBe(setupKey);expect(seen[2]).toEqual(seen[0]);
  expect(seen[1]?.key).toBe(registerKey);expect(seen[3]).toEqual(seen[1]);
  expect(adopt).not.toHaveBeenCalled();
  expect(client.session.getState()).toBe(original);
});
