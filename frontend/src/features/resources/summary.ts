/** The landing and navigation share one native aggregate for the same committed screen opening. */
import { useQuery } from '@tanstack/react-query';
import { useContext } from 'react';
import { resourceSummaryQuery } from '@/api/resource-summary';
import { ViewRouteContext } from '@/features/workbench/viewOpening';
import { openingId } from '@/lib/opening';

export function useResourceSummary() {
  const route = useContext(ViewRouteContext);
  return useQuery(resourceSummaryQuery(route ?? openingId()));
}
