/** Regional gateway reports shared by component proofs and the routed screen gate. */
import type { TunnelGatewayDecision } from '../../src/features/gateways/placement';
import type { ConnectorGatewayStatus, GatewayCatalogue, ThreadTunnelGatewayStatus } from '../../src/generated/api-types.ts';

export const catalogue: GatewayCatalogue = { revision: 'catalogue-before', gateways: [
  { id: 'eu', label: 'Europe gateway', region: 'Europe', endpoints: [
    { url: 'wss://eu.gateway.example.test:9443', trust: 'pinned' },
    { url: 'wss://europe-public.gateway.example.test/link', trust: 'public' },
  ] },
  { id: 'asia', label: 'Asia tools', region: 'Asia', endpoints: [{ url: 'wss://asia.gateway.example.test:9443', trust: 'pinned' }] },
] };
export const connector: ConnectorGatewayStatus = {
  machine_id: '00000000-0000-4000-8000-000000000001', selection: 'automatic', selected_gateway_id: null,
  primary_gateway_id: 'asia', connected: true, measured_at: '2026-11-06T10:00:00Z',
  measurements: [
    { gateway_id: 'eu', endpoint: catalogue.gateways[0]!.endpoints[0]!, round_trip_ms: 190 },
    { gateway_id: 'eu', endpoint: catalogue.gateways[0]!.endpoints[1]!, round_trip_ms: 205 },
    { gateway_id: 'asia', endpoint: catalogue.gateways[1]!.endpoints[0]!, round_trip_ms: 18 },
  ],
};
export const tunnel: Omit<ThreadTunnelGatewayStatus, 'plan'> & { plan: NonNullable<ThreadTunnelGatewayStatus['plan']> & { decision: TunnelGatewayDecision } } = {
  thread_id: '00000000-0000-4000-8000-000000000002', state: 'attached', source_primary_gateway_id: 'eu',
  destination_primary_gateway_id: 'asia', attached_gateway_id: 'asia', refusal: null,
  plan: { plan_id: '00000000-0000-4000-8000-000000000003', src: connector.machine_id,
    dst: '00000000-0000-4000-8000-000000000004', preferred_gateway: catalogue.gateways[1]!,
    common_fallbacks: [catalogue.gateways[0]!], catalogue_revision: catalogue.revision,
    decision: { rule: 'lowest_combined_rtt', source_measured_at: connector.measured_at, destination_measured_at: connector.measured_at, max_sample_age_secs: 60, combined_round_trip_ms: 208 } },
};
