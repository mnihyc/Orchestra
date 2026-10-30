/** Switching to another thing at the same route resets local state across the route tree. */
import { act, render, screen } from '@testing-library/react';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider, useParams } from '@tanstack/react-router';
import { useState } from 'react';
import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { routeParams } from '@/lib/routeIdentity';

function Detail() {
  const params = useParams({ strict:false });
  const [opened] = useState(params.id);
  return <input aria-label="Opened thing" value={opened ?? ''} readOnly />;
}
const files = (directory:string):string[] => readdirSync(directory,{ withFileTypes:true }).flatMap((entry) => entry.isDirectory() ? files(resolve(directory,entry.name)) : entry.name.endsWith('.tsx') ? [resolve(directory,entry.name)] : []);

describe('a route opens the new thing', () => {
  it('drops a previous provider’s local draft when navigating to another provider', async () => {
    const root = createRootRoute({ component:Outlet });
    const detail = createRoute({ getParentRoute:() => root,path:'/resources/providers/$id',component:Detail });
    const router = createRouter({ routeTree:root.addChildren([detail]),history:createMemoryHistory({ initialEntries:['/resources/providers/first'] }),defaultRemountDeps:routeParams,defaultPendingMinMs:0 });
    render(<RouterProvider router={router} />);
    expect(await screen.findByLabelText('Opened thing')).toHaveValue('first');
    await act(async () => { await router.navigate({ to:'/resources/providers/$id',params:{ id:'second' } }); });
    expect(screen.getByLabelText('Opened thing')).toHaveValue('second');
  });
  it('keeps every parameter route under the shared rule, including future routes', () => {
    expect(readFileSync(resolve('src/app.tsx'),'utf8')).toContain('defaultRemountDeps: routeParams');
    const routes = files(resolve('src/routes')).filter((path) => path.includes('$'));
    expect(routes.length).toBeGreaterThan(0);
    for (const path of routes) expect(readFileSync(path,'utf8'),path).not.toMatch(/\bremountDeps\s*:/);
  });
});
