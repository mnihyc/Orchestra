/** Display only reported policy evidence; missing measurements never become zero. */
export interface TunnelGatewayDecision {
  rule: 'lowest_combined_rtt' | 'executor_pinned' | 'executor_samples_unavailable' | 'executor_peer_capability';
  source_measured_at: string | null;
  destination_measured_at: string | null;
  max_sample_age_secs: number;
  combined_round_trip_ms: number | null;
}
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null;
export function placementDecision(plan: unknown): TunnelGatewayDecision | null {
  if (!record(plan) || !record(plan.decision)) return null;
  const value = plan.decision, rule = value.rule;
  if (rule !== 'lowest_combined_rtt' && rule !== 'executor_pinned' && rule !== 'executor_samples_unavailable' && rule !== 'executor_peer_capability') return null;
  if (typeof value.max_sample_age_secs !== 'number' || !Number.isFinite(value.max_sample_age_secs) || value.max_sample_age_secs < 0) return null;
  return { rule, max_sample_age_secs: value.max_sample_age_secs,
    source_measured_at: typeof value.source_measured_at === 'string' ? value.source_measured_at : null,
    destination_measured_at: typeof value.destination_measured_at === 'string' ? value.destination_measured_at : null,
    combined_round_trip_ms: typeof value.combined_round_trip_ms === 'number' && Number.isFinite(value.combined_round_trip_ms) && value.combined_round_trip_ms >= 0 ? value.combined_round_trip_ms : null };
}
