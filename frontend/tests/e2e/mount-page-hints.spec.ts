/** A mount's page says what plans preselect for it, what each location's data is, and the data a copy displaced. */
import { expect, test } from '@playwright/test';
import words from '../../src/i18n/en.json' with { type: 'json' };
import type { CleanupPlan, Mount } from '../../src/generated/api-types';
import { ACME, RUNS_ON } from '../doubles/fixtures';
import { ADA } from '../doubles/thread-scenes';
import {storageCleanupPlan,storageCleanupResult,STORAGE_CLEANUP_JOB} from '../doubles/storage-report';
import { shot } from './change-support';
import { scenario, signIn } from './support';

const MOUNT = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a62';
const KEPT = '0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a63';
const home = { id: ACME.org, kind: 'org' as const, name: 'Acme', redacted: false };
const machine = { id: RUNS_ON.laptop, name: 'host-1', online: true, platform_os: 'linux' as const, placement: 'managed' as const };
const host = { id: RUNS_ON.buildHost, name: 'host-1', address: 'host-1' };

test.beforeEach(async () => { await scenario(); });

for (const width of [1280, 390]) {
  test(`a mount's page shows its default, a location's hint and kept data that is purged once named (${width}px)`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    const mount: Mount = {
      id: MOUNT, name: 'datasets', address: 'acme.ada.datasets', description: 'Shared data', org_id: ACME.org, owner: ADA, home,
      target: '/mnt/data', read_only: true, enabled: true, default_fate: 'copy', revision: 3,
      access: { admin_only: [], participating: true, permissions: ['mount.view', 'mount.use', 'mount.manage'], administers: false, through: [{ kind: 'owner' }] },
      locations: [{ machine, host, kind: 'volume', storage: null, source: `orch-mount-${MOUNT}-3e4f5a6b7c8d`, data_kind: 'synced', can_remove: true, created_by: ADA, created_at: '2026-10-14T08:00:00Z' }],
      kept: [{ id: KEPT, machine, host, kind: 'volume', storage: null, source: `orch-mount-${MOUNT}-3e4f5a6b7c8d-kept-0a1b2c3d4e5f`, data_kind: 'own',
        file_count: 1200, total_bytes: 2147483648, kept_by: ADA, kept_at: '2026-10-20T09:00:00Z', can_purge: true }],
    };
    const hints: unknown[] = [];
    let purged = '';
    let accepted=0;
    const publish=new Set<()=>void>();
    const base=storageCleanupPlan(['recovery']);
    const first=base.items[0];if(first===undefined)throw new Error('The cleanup fixture has retained files');
    const plan:CleanupPlan={...base,confirmation_name:mount.name,items:[{...first,record:{...first.record,id:`mount:${KEPT}`,kind:'mount_data',name:mount.name,resource:{kind:'mount',id:MOUNT},owner:ADA,host,place:home,bytes:2147483648,accounting_key:`mount:${KEPT}`,why:'restore',until:null,facts:null}}]};
    const result=storageCleanupResult(plan,new Map([[`mount:${KEPT}`,'purge']]));
    await page.route(/\/api\/v1\/storage\/cleanup(?:\/preflight)?$/u,async(route)=>{
      const body:unknown=route.request().postDataJSON();
      if(new URL(route.request().url()).pathname.endsWith('/preflight')){
        expect(body).toEqual({scope:'me',resource:{kind:'mount',id:MOUNT},items:[`mount:${KEPT}`]});
        await route.fulfill({status:200,json:plan});
      }else{
        expect(body).toEqual({plan_id:plan.id,choices:[{id:`mount:${KEPT}`,choice:'purge'}],confirmation_name:mount.name,notify:false});
        accepted+=1;purged=mount.name;mount.kept=[];for(const send of publish)send();
        await route.fulfill({status:202,json:result});
      }
    });
    await page.route(`**/api/v1/jobs/${STORAGE_CLEANUP_JOB}`,(route)=>route.fulfill({status:200,json:result}));
    await page.route('**/api/v1/mounts**', async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      if (request.method() === 'PATCH' && url.pathname.endsWith(`/locations/${machine.id}`)) {
        const body = request.postDataJSON() as { data_kind: Mount['locations'][number]['data_kind'] };
        hints.push(body.data_kind);
        mount.locations = mount.locations.map((location) => ({ ...location, data_kind: body.data_kind }));
        await route.fulfill({ json: mount });
        return;
      }
      expect(request.method(),'kept data uses the common cleanup job rather than the retired direct purge').not.toBe('DELETE');
      if (url.pathname.endsWith('/uses')) { await route.fulfill({ json: { scopes: [], environments: [], hidden: 0 } }); return; }
      await route.fulfill({ json: url.pathname.endsWith('/mounts') ? [{ ...mount, content_hidden: false }] : mount });
    });
    // The fake API does not know this mount, so the page's feed of it is answered here: followed, and quiet.
    const feed = `resource:mount:${MOUNT}`;
    await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/, (ws) => {
      const server = ws.connectToServer();
      const answered = new Set<number>();
      const send=()=>{for(const id of answered)ws.send(JSON.stringify({op:'event',id,feed,data:{row:mount}}));};publish.add(send);
      ws.onMessage((raw) => {
        const value = JSON.parse(String(raw)) as { op: string; id?: number; feed?: string };
        if (value.op === 'sub' && value.feed === feed && value.id !== undefined) {
          answered.add(value.id);
          ws.send(JSON.stringify({ op: 'subscribed', id: value.id, feed, seq: null, mode: 'ephemeral' }));
        } else if (value.op === 'unsub' && value.id !== undefined && answered.delete(value.id)) {
          ws.send(JSON.stringify({ op: 'closed', id: value.id, feed, reason: 'unsubscribed' }));
        } else server.send(raw);
      });
      ws.onClose((code, reason) => { publish.delete(send); void server.close({ ...(code === undefined ? {} : { code }), ...(reason === undefined ? {} : { reason }) }); });
    });
    await signIn(page);
    await page.goto(`/resources/mounts/${MOUNT}`);
    await expect(page.getByRole('heading', { name: 'datasets', exact: true })).toBeVisible();
    await expect(page.locator('[data-resource-facts] > div', { hasText: words.resourceDetails.mount.defaultFate }))
      .toContainText(words.choiceCatalog.mountFate.copy.name);
    await shot(page, `mount-page-1-overview-${width}`);
    await page.getByRole('tab', { name: words.resourceDetails.mount.locations, exact: true }).click();
    const locations = page.getByRole('region', { name: words.resourceDetails.mount.locations, exact: true });
    const hint = locations.getByRole('combobox', { name: words.resourceDetails.mount.dataKind, exact: true });
    await expect(hint).toContainText(words.choiceCatalog.mountDataKind.synced.name);
    const kept = page.locator('[data-mount-kept]');
    await expect(kept).toContainText(words.resourceDetails.mount.kept);
    await expect(kept).toContainText(`orch-mount-${MOUNT}-3e4f5a6b7c8d-kept-0a1b2c3d4e5f`);
    await expect(kept).toContainText(/1,?200 files/);
    await shot(page, `mount-page-2-locations-${width}`);
    await hint.click();
    await page.getByRole('option', { name: words.choiceCatalog.mountDataKind.own.name }).click();
    await expect.poll(() => hints).toEqual(['own']);
    await expect(hint).toContainText(words.choiceCatalog.mountDataKind.own.name);
    await kept.getByRole('button', { name: words.resourceDetails.mount.purge, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words.storage.clean, exact: true });
    const purge = dialog.getByRole('button', { name: words.storage.clean, exact: true });
    await expect(purge).toBeDisabled();
    await dialog.getByRole('combobox',{name:words.storage.cleanup.fateFor.replace('{{name}}',mount.name),exact:true}).click();
    await page.getByRole('option',{name:words.storage.cleanup.choices.purge,exact:true}).click();
    await expect(purge).toBeDisabled();expect(accepted).toBe(0);
    const confirmation=dialog.getByRole('textbox',{name:words.storage.cleanup.typeName.replace('{{name}}',mount.name),exact:true});
    await confirmation.fill('Datasets');await expect(purge).toBeDisabled();expect(accepted).toBe(0);
    await confirmation.fill('datasets');await expect(purge).toBeEnabled();
    await shot(page, `mount-page-3-purge-${width}`);
    await purge.click();
    await expect.poll(() => purged).toBe('datasets');expect(accepted).toBe(1);
    await expect(dialog.getByRole('table',{name:words.storage.cleanup.results,exact:true}).getByText(words.storage.cleanup.outcomes.removed,{exact:true})).toBeVisible();
    await expect(page.locator('[data-mount-kept]')).toHaveCount(0);
  });
}
