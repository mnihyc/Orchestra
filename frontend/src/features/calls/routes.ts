/** Capability route candidates retain the existing calls.rules wire shape and private backend data. */
import declarations from '../../../../shared/capabilities.json';
export interface RouteChoice { route: string; offering?: { provider: string; offering: string } | null; config?: unknown }
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
export function candidateRoute(value: unknown): RouteChoice | null {
  if (!record(value) || !record(value.route) || typeof value.route.route !== 'string') return null;
  const route = value.route;
  if (route.offering !== undefined && route.offering !== null && (!record(route.offering) || typeof route.offering.provider !== 'string' || typeof route.offering.offering !== 'string')) return null;
  return route as unknown as RouteChoice;
}
export function capabilityOfPurpose(purpose: string | undefined): string | null {
  return purpose !== undefined ? Object.entries(declarations.capabilities).find(([, capability]) => 'recovery_purpose' in capability && capability.recovery_purpose === purpose)?.[0] ?? null : null;
}
export function routeChoices(purpose: string | undefined, locale: string): { value: string; label: string }[] {
  const capability = capabilityOfPurpose(purpose);
  if (capability === null) return [];
  const declared = declarations.capabilities[capability as keyof typeof declarations.capabilities];
  return declared.modes.service.routes.map((id) => {
    const route = declarations.routes[id as keyof typeof declarations.routes];
    const text: Readonly<Record<string, string | undefined>> = route.text;
    return { value: id, label: text[locale] ?? route.title };
  });
}
