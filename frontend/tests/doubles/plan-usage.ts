/** Deliberately different plan windows for visual checks of forecasts, stopped readings and early resets. */
import type { Provider, ProviderPlanSeries, UsageBucket, UsageWindow } from '../../src/generated/api-types.ts';
const HOUR=3_600_000;
const DAY=24*HOUR;

export interface PlanSeed {
  facts:Provider['facts'];
  forecasts:NonNullable<Provider['plan_forecasts']>;
  rows:ProviderPlanSeries[];
  at:string;
}

export function seedPlan(facts:Provider['facts'],now:number):PlanSeed {
  const stamp=(at:number)=>new Date(at).toISOString();
  const window=(id:string,label:string,duration:number,used:number,left:number,family:string | null=null):UsageWindow=>({id,label,duration_minutes:duration,used_percent:used,resets_at:stamp(now+left),model_family:family,is_active:id==='five',reset_not_started:false,warning_percent:null,severity:null});
  const windows=[window('five','5-hour window',300,60,2*HOUR),window('week','Weekly',10080,20,5*DAY,'Opus'),window('reached','At the limit',300,100,2*HOUR),window('stale','Stopped readings',300,35,HOUR),window('vendor','Weekly · vendor reset',10080,5,6*DAY),window('outside','Weekly · reset elsewhere',10080,4,6*DAY)];
  const forecasts:PlanSeed['forecasts']={five:{basis:'hour',percent_at_end:140,runs_out_at:stamp(now+HOUR)},week:{basis:'day',percent_at_end:70,runs_out_at:null},reached:{basis:'hour',percent_at_end:100,runs_out_at:stamp(now)},vendor:{basis:'window',percent_at_end:65,runs_out_at:null},outside:{basis:'window',percent_at_end:52,runs_out_at:null}};
  const field_times={...facts.field_times,windows:stamp(now),'windows.stale':stamp(now-3*HOUR)};
  const rows=windows.map((window):ProviderPlanSeries=>{
    const early=window.id==='vendor' || window.id==='outside';
    const start=early ? now-12*HOUR : Date.parse(window.resets_at ?? stamp(now))-(window.duration_minutes ?? 0)*60_000;
    const end=window.id==='stale' ? now-3*HOUR : now;
    const count=window.duration_minutes===300 ? 36 : 48;
    const points=Array.from({length:count+1},(_,index)=>{
      const share=index/count;
      const value=window.id==='five' ? share<=2/3 ? share*30 : 20+(share-2/3)*120 : window.used_percent*share;
      return {at:stamp(start+(end-start)*share),used_percent:value};
    });
    return {window_id:window.id,starts_at:stamp(start),points,history:[],resets:early ? [{id:`00000000-0000-4000-8000-${window.id==='vendor' ? '000000000031' : '000000000032'}`,at:stamp(start),cause:window.id==='vendor' ? 'vendor' : 'outside',count:1}] : [],current_reset:null,summary:{mean_used_percent:null,limit_buckets:0,early_resets:0}};
  });
  return {facts:{...facts,plan:'Team plan',windows,field_times},forecasts,rows,at:stamp(now)};
}

function bounds(at:number,bucket:UsageBucket):[number,number] {
  const d=new Date(at);
  let start:number,end:number;
  if(bucket==='hour') {start=Math.floor(at/HOUR)*HOUR;end=start+HOUR;}
  else if(bucket==='day') {start=Date.UTC(d.getUTCFullYear(),d.getUTCMonth(),d.getUTCDate());end=start+DAY;}
  else if(bucket==='week') {start=Date.UTC(d.getUTCFullYear(),d.getUTCMonth(),d.getUTCDate())-((d.getUTCDay()+6)%7)*DAY;end=start+7*DAY;}
  else {start=Date.UTC(d.getUTCFullYear(),d.getUTCMonth(),1);end=Date.UTC(d.getUTCFullYear(),d.getUTCMonth()+1,1);}
  return [start,end];
}

export function planReply(seed:PlanSeed | undefined,windows:readonly UsageWindow[],from:string,to:string,bucket:UsageBucket):ProviderPlanSeries[] {
  const now=seed===undefined ? Date.now() : Date.parse(seed.at);
  const since=Math.max(Date.parse(`${from}T00:00:00Z`),now-30*DAY);
  const until=Math.min(Date.parse(`${to}T00:00:00Z`)+DAY,now);
  const byId=new Map(seed?.rows.map((row)=>[row.window_id,row]));
  return windows.map((window)=>{
    const current=byId.get(window.id) ?? {window_id:window.id,starts_at:null,points:[],history:[],resets:[],current_reset:null,summary:{mean_used_percent:null,limit_buckets:0,early_resets:0}};
    if(seed===undefined) return current;
    const history:ProviderPlanSeries['history']=[];
    for(let at=since;at<until;) {
      const [start,end]=bounds(at,bucket);
      const rate=window.duration_minutes===300 ? 75 : 10;
      const amount=rate*(Math.min(end,until)-Math.max(start,since))/DAY;
      history.push({at:new Date(start).toISOString(),ends_at:new Date(end).toISOString(),used_percent:amount,reached_limit:window.id==='reached' || window.id==='five' && Math.floor(start/DAY)%7===0});
      at=end;
    }
    const resets=current.resets.filter((reset)=>Date.parse(reset.at)>=since && Date.parse(reset.at)<until);
    const mean=history.length===0 ? null : history.reduce((sum,row)=>sum+row.used_percent,0)/history.length;
    return {...current,history,resets,current_reset:current.resets.at(-1) ?? null,summary:{mean_used_percent:mean,limit_buckets:history.filter((row)=>row.reached_limit).length,early_resets:resets.reduce((sum,row)=>sum+row.count,0)}};
  });
}
