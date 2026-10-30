/** Device-local usage choices, with explicit addresses taking precedence and filters staying in the address alone. */
import { useCallback, useMemo } from 'react';
import type { UsageDimension } from '@/generated/api-types';
import { useRememberedView } from '@/lib/viewChoices';
import { DEFAULT_MEASURE, DEFAULT_RANGE, DEFAULT_SPLIT, RANGES, type UsageSearch, type Measure, type Split } from './search';

export type SearchPatch = { [Key in keyof UsageSearch]?: UsageSearch[Key] | undefined };
const VIEWS = ['usage', 'performance', 'requests'] as const;
const USAGE_MEASURES: readonly Measure[] = ['turns', 'requests', 'tokens', 'credits'];
const PERFORMANCE_MEASURES: readonly Measure[] = ['speed', 'first_token', 'errors'];
const SPLITS: readonly Split[] = ['person','model','provider','agent','environment','effort','tier','program','org','project','topic','thread','token_kind','none'];
const DIMENSIONS = ['person','model','provider','agent','environment','effort','tier','program','org','project','topic','thread'] as const;
const BUCKETS = ['auto','hour','day','week','month'] as const;
const PLAN_MODES = ['current','history'] as const;

/** Each surface has its own key; each chart's choices survive a visit to another chart. */
export function useUsageChoices(raw: UsageSearch, kind: 'usage' | 'providerUsage',defaultTable:UsageDimension): [UsageSearch, (patch: SearchPatch) => void] {
  const [view, keepView] = useRememberedView(kind,'tab',raw.view,'usage',VIEWS);
  const chart = view === 'performance' ? 'performance' : 'overTime';
  const [show, keepShow] = useRememberedView<Measure>(kind,`${chart}.measure`,raw.show,view === 'performance' ? 'speed' : DEFAULT_MEASURE,view === 'performance' ? PERFORMANCE_MEASURES : USAGE_MEASURES);
  const [series, keepSeries] = useRememberedView<Split>(kind,`${chart}.split`,raw.series,DEFAULT_SPLIT,SPLITS);
  const [bucket, keepBucket] = useRememberedView(kind,`${chart}.bucket`,raw.bucket,'auto',BUCKETS);
  const [plan, keepPlan] = useRememberedView(kind,'plan.mode',raw.plan,'current',PLAN_MODES);
  const [planBucket, keepPlanBucket] = useRememberedView(kind,'plan.bucket',raw.plan_bucket,'auto',BUCKETS);
  const [by, keepBy] = useRememberedView(kind,'breakdown',raw.by,defaultTable,DIMENSIONS);
  const [range, keepRange] = useRememberedView(kind,'period',raw.range,DEFAULT_RANGE,RANGES);
  const keep = useCallback((patch: SearchPatch) => {
    if (patch.view !== undefined) keepView(patch.view);
    // Let the next view resume its own choices instead of storing this view's cleared address fields.
    if (patch.view === undefined) {
      if (patch.show !== undefined) keepShow(patch.show);
      if (patch.series !== undefined) keepSeries(patch.series);
      if (patch.bucket !== undefined) keepBucket(patch.bucket);
    }
    if (patch.plan !== undefined) keepPlan(patch.plan);
    if (patch.plan_bucket !== undefined) keepPlanBucket(patch.plan_bucket);
    if (patch.by !== undefined) keepBy(patch.by);
    if (patch.range !== undefined) keepRange(patch.range);
  },[keepView,keepShow,keepSeries,keepBucket,keepPlan,keepPlanBucket,keepBy,keepRange]);
  const search=useMemo(()=>({...raw,view,show,series,bucket,plan,plan_bucket:planBucket,by,range}),[raw,view,show,series,bucket,plan,planBucket,by,range]);
  return [search,keep];
}
