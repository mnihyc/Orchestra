/** Hovering a destination fetches its code without executing admission or data loaders. */
import { createMemoryHistory, createRootRoute, createRoute, createRouter } from '@tanstack/react-router';
import { expect, it, vi } from 'vitest';
import { prefetchScreenCode } from '@/shell/screenCode';

it('prepares matched code only, preserving pathname and query', () => {
  const beforeLoad = vi.fn<() => void>();
  const loader = vi.fn<() => void>();
  const root = createRootRoute();
  const route = createRoute({ getParentRoute: () => root, path: '/resources/providers', beforeLoad, loader });
  const router = createRouter({ routeTree: root.addChildren([route]), history: createMemoryHistory({ initialEntries: ['/'] }) });
  const matchRoutes = vi.spyOn(router, 'matchRoutes');
  const loadRouteChunk = vi.spyOn(router, 'loadRouteChunk').mockImplementation(() => Promise.resolve());
  prefetchScreenCode(router, '/resources/providers?q=Claude');
  expect(matchRoutes).toHaveBeenCalledWith('/resources/providers', { q: 'Claude' });
  expect(loadRouteChunk).toHaveBeenCalledWith(route);
  expect(loadRouteChunk).toHaveBeenCalledWith(root);
  expect(beforeLoad).not.toHaveBeenCalled();
  expect(loader).not.toHaveBeenCalled();
  prefetchScreenCode(router, 'https://somewhere.invalid/resource');
  prefetchScreenCode(router, 'mailto:reader@example.invalid');
  expect(loadRouteChunk).toHaveBeenCalledTimes(2);
});
