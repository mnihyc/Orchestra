/** Native provider observations for card, privacy and guarded-reset tests. */
import { maybeHarnessName } from '../doubles/harness-names';
import type { PlanProvider } from '@/features/resources/providers/plan';

export const PLAN_NOW = Date.parse('2026-10-14T12:00:00.000Z');
export const OBSERVED = '2026-10-14T11:59:00.000Z';

export const SOURCE_DESCRIPTOR: NonNullable<PlanProvider['sources']['facts']['descriptor']> = { name:'adapter',title:'Example source',summary:'Reports the example account.',contract:1,
  facts:{reports:['plan','windows','limit_reached','ordinary_usage_allowed','balances','spend','extra_usage','available_resets','identity','credential','credential_verified'],spends_resets:true,credential:'none',min_interval_secs:60,plans:{Pro:'Pro'}},
  models:{},presents:null,fields:[],docs_url:null };

export function planProvider(patch: Partial<PlanProvider> = {}): PlanProvider {
  return {
    id: 'plan-provider', name: 'Team Pro', kind: 'subscription', address: 'acme.team.team-pro', placement: 'managed', status: 'ready',credential_keeper:patch.credential_keeper ?? patch.key_machine ?? null,
    status_detail: null, status_at: OBSERVED, facts_at: OBSERVED, facts_requested_at: OBSERVED,
    sources:{facts:{by:'default',name:'adapter',address:null,config:{},secrets:[],descriptor:structuredClone(SOURCE_DESCRIPTOR),described_at:OBSERVED,last_error:null},models:{by:'default',name:'adapter',address:null,config:{},secrets:[],descriptor:structuredClone(SOURCE_DESCRIPTOR),described_at:OBSERVED,last_error:null},presenter:null},
    facts_state:{identity:'told',credential:'told',credential_verified:'told',plan:'told',windows:'told',limit_reached:'empty',ordinary_usage_allowed:'not_read_yet',balances:'told',spend:'empty',extra_usage:'empty',available_resets:'told',models:'not_read_yet'},
    owner: { id: 'ada', kind:'human', display_name: 'Ada Lovelace', username: 'ada', avatar:{ text:'A',color:1 } }, access: { permissions: ['provider.view', 'provider.use', 'provider.manage'], administers: false, through: [],admin_only:[],participating:true },
    owners:1,org_id:'org',home:{ id:'org',kind:'org',name:'Acme',redacted:false },agent:null,harness_kind:'fake',arrangements:[],held_by:null,runtime_config:{},upstreams:[],secrets_set:[],config:{},subscription:null,protocols:[],key_set:false,key_machine:null,machine:null,facts_overridden:[],models:[],running_threads:0,runtime:null,harnesses:[],created_at:OBSERVED,updated_at:OBSERVED,
    facts: { models: null, plan: 'Pro', windows: [{ id: 'primary', label: '5-hour window', used_percent: 0, duration_minutes: 300, resets_at: null, reset_not_started: true, is_active: false, model_family:null, warning_percent:null, severity:null }, { id: 'weekly', label: 'Weekly window', used_percent: 81, duration_minutes: 10080, resets_at: '2026-10-17T12:00:00.000Z', is_active: true, model_family: 'Fable', reset_not_started:null, warning_percent:null, severity:null }], balances:[{id:'plan',label:'Plan credits',unit:'vendor credits',remaining:'12.4',total:null,used:null,unlimited:false,parts:[],expires_at:null}], spend:[], identity: 'team@example.org', credential: { method: 'access_token', yields: 'token', issued_at: '2026-10-13T12:00:00.000Z', expires_at: '2026-10-23T12:00:00.000Z' }, limit_reached: null, extra_usage:null, ordinary_usage_allowed:null, credential_verified:true, field_times: { plan: OBSERVED, windows: OBSERVED, balances: OBSERVED, spend:OBSERVED,limit_reached:OBSERVED,extra_usage:OBSERVED,identity: OBSERVED, credential: OBSERVED,credential_verified:OBSERVED,available_resets: OBSERVED }, available_resets: { available_count: 2, credits: [{ id: 'reset-a', title: 'Full reset', description: null, reset_type: 'codexRateLimits', status: 'available', granted_at: OBSERVED, expires_at: '2026-10-23T12:00:00.000Z' }] } },
    revision: 1, last_reset: null, ...patch,
    harness: patch.harness === undefined ? maybeHarnessName(patch.harness_kind === undefined ? 'fake' : patch.harness_kind) : patch.harness,
  };
}
