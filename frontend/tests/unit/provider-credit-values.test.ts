/** Reported decimals remain exact text; only representable chart coordinates become numbers. */
import { expect,it } from 'vitest';
import type { ProviderCreditPoint,ProviderCreditSeries } from '@/generated/api-types';
import { detailFieldVisible,scaledDecimal,scaledAmountText } from '@/features/resources/providers/details';
import { creditCoordinate,creditGroups } from '@/features/usage/CreditChart';
import { planProvider } from './provider-plan-fixtures';

const point:ProviderCreditPoint={at:'2026-11-03T12:00:00Z',amount:'9007199254740993.123456789',unlimited:null,enabled:true,gap_before:false};
const series:ProviderCreditSeries={kind:'credits',series_id:'used',group_id:'account-a-usd',member:'extra_usage',item_id:'',quantity:'used',label:'Paid usage',unit:'USD',decimal_places:2,points:[point]};

it('moves the source decimal point without a float roundtrip or truncation',()=>{
  expect(scaledDecimal(point.amount!,2)).toBe('90071992547409.93123456789');
  expect(scaledDecimal('1234',2)).toBe('12.34');
  expect(scaledDecimal('2000',2)).toBe('20');
  expect(scaledDecimal('-0.1234',2)).toBe('-0.001234');
  expect(scaledAmountText('0','USD',2,'Unknown','Unit unknown')).toBe('0 USD');
  expect(scaledAmountText(null,'USD',2,'Unknown','Unit unknown')).toBe('Unknown');
  expect(scaledAmountText('2',null,null,'Unknown','Unit unknown')).toBe('2 Unit unknown');
});

it('keeps huge scale explicit without padding or plotting underflow as zero',()=>{
  expect(scaledDecimal('12',4_294_967_295)).toBe('12 × 10^−4294967295');
  expect(creditCoordinate({...point,amount:'1'},{...series,decimal_places:4_294_967_295})).toBeNull();
  expect(creditCoordinate({...point,amount:'0'},{...series,decimal_places:4_294_967_295})).toBe(0);
  expect(creditCoordinate({...point,amount:null},series)).toBeNull();
  expect(creditCoordinate({...point,unlimited:true},series)).toBeNull();
  expect(creditCoordinate({...point,enabled:false},series)).toBeNull();
});

it('hides only source-resolved unsupported fields and retains legacy missing quantities',()=>{
  const provider=planProvider();
  expect(detailFieldVisible(provider,'balances.plan.total')).toBe(true);
  provider.facts_state.fields={'balances.plan.total':'unsupported','balances.plan.used':'empty','balances.plan.remaining':'told'};
  expect(detailFieldVisible(provider,'balances.plan.total')).toBe(false);
  expect(detailFieldVisible(provider,'balances.plan.used')).toBe(true);
  expect(detailFieldVisible(provider,'balances.plan.remaining')).toBe(true);
});

it('overlays only the server-proven source/item/unit group',()=>{
  const limit={...series,series_id:'limit',quantity:'limit' as const};
  const differentUnit={...series,series_id:'eur',group_id:'account-a-eur',unit:'EUR'};
  const differentSource={...series,series_id:'another',group_id:'account-b-usd'};
  expect(creditGroups([series,limit,differentUnit,differentSource])).toEqual([[series,limit],[differentUnit],[differentSource]]);
});
