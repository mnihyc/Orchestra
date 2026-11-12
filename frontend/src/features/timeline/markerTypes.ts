/** Native harness observations use the same timeline renderer as stored history markers. */
import type { MarkerPayload as StoredMarkerPayload } from '@/generated/api-types';

type Measurement = {state:'unsupported'|'unread'|'empty'} | {state:'told';value:unknown};
export interface HarnessChangedMarker {
  marker:'harness_changed';
  data:{
    program:string;
    change:string;
    subject?:string|null;
    previous:Measurement;
    value:unknown;
    expected:Measurement;
    changed?:boolean;
    diverged:boolean;
    next_turn:boolean;
    turn_id:string|null;
    reported_at:string;
  };
}
export type MarkerPayload = StoredMarkerPayload | HarnessChangedMarker;
