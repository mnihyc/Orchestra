/** Permission expiry makes a new observation even when the previous response arrived late, without a retry storm. */
import {expect,it} from 'vitest';
import {referenceRefreshDelay} from '@/api/references';
const start=Date.parse('2026-10-26T12:00:00Z');
const end=new Date(start+1000).toISOString();
it('schedules the known deadline from the observation that began before it',()=>{
 expect(referenceRefreshDelay({refresh_at:end,read_started_at:start},start+20,0,start+500)).toBe(500);
});
it('a late response still needs its one fresh observation after the deadline',()=>{
 expect(referenceRefreshDelay({refresh_at:end,read_started_at:start},start+2000,0,start+2000)).toBe(1);
 expect(referenceRefreshDelay({refresh_at:end,read_started_at:start+2000},start+2020,0,start+2020)).toBe(false);
});
it('an error after the deadline stops automatic retries and leaves the card retry in charge',()=>{
 expect(referenceRefreshDelay({refresh_at:end,read_started_at:start},start+20,start+2000,start+2020)).toBe(false);
 expect(referenceRefreshDelay({refresh_at:null},start+20,0,start+2020)).toBe(false);
});
