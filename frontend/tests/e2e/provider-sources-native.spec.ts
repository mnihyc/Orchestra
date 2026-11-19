/** The registered Acme example is chosen and read through the real source route, with no constructed reports. */
import { readFileSync } from 'node:fs';
import { expect, test } from '@playwright/test';
import type { Provider } from '../../src/generated/api-types';

interface ProviderSourcesWalk {
  /** English manager session for a disposable, ready key provider with a running keeper. */
  storageState:string;
  provider:string;
  /** The registered plan.py program's current offered label, after its first describe. */
  programLabel:string;
}
const manifest=process.env.ORCH_PROVIDER_SOURCES_WALK;
const walk=manifest === undefined ? undefined : JSON.parse(readFileSync(manifest,'utf8')) as ProviderSourcesWalk;
// Two describes (10 s each), facts (8 s) and model listing (30 s), plus navigation and visible reads.
const SOURCE_PROOF_TIMEOUT_MS=90_000;
const DEMO_SECRET='source-proof-not-a-credential';

test.describe('real provider sources', () => {
  test.skip(walk === undefined || process.env.ORCH_NATIVE_URL === undefined,'The integration runner supplies ORCH_PROVIDER_SOURCES_WALK and an installed Acme example program on the keeping machine/template.');
  test('a key reports windows, money and resets from the program; its models are offered and its secret stays write-only', async ({browser}) => {
    test.setTimeout(SOURCE_PROOF_TIMEOUT_MS);
    if (walk === undefined) return;
    const context=await browser.newContext({baseURL:process.env.ORCH_NATIVE_URL,storageState:walk.storageState,viewport:{width:1280,height:900},locale:'en-US',timezoneId:'UTC'});
    try {
      const page=await context.newPage();
      const beforeResponse=await context.request.get('/api/v1/providers/' + walk.provider);
      expect(beforeResponse.ok()).toBe(true);
      const before=await beforeResponse.json() as Provider;
      const enabledBefore=before.models.map((model) => [model.model_name,model.enabled]).sort();
      const doubles:string[]=[];
      const setupContexts:string[]=[];
      page.on('request',(request) => {const url=new URL(request.url());if (url.pathname.startsWith('/__double')) doubles.push(request.url());if (url.pathname.endsWith('/provider-setup')) setupContexts.push(url.searchParams.get('provider_id') ?? '');});
      await page.goto('/resources/providers/' + walk.provider + '?tab=settings');
      const facts=page.getByRole('group',{name:'Details from',exact:true});
      await facts.getByRole('combobox',{name:'Details from',exact:true}).click();
      await page.getByRole('option',{name:'Program',exact:true}).click();
      await facts.getByRole('combobox',{name:'Registered program',exact:true}).click();
      await page.getByRole('option',{name:walk.programLabel,exact:true}).click();
      await facts.getByLabel(/Example secret/).fill(DEMO_SECRET);
      const models=page.getByRole('group',{name:'Models from',exact:true});
      await models.getByRole('combobox',{name:'Models from',exact:true}).click();
      await page.getByRole('option',{name:'Program',exact:true}).click();
      await models.getByRole('combobox',{name:'Registered program',exact:true}).click();
      await page.getByRole('option',{name:walk.programLabel,exact:true}).click();
      const written=page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname.endsWith('/providers/' + walk.provider + '/sources'));
      await page.getByRole('button',{name:'Save sources',exact:true}).click();
      const response=await written;
      expect(response.ok()).toBe(true);
      const text=await response.text();
      expect(text).not.toContain(DEMO_SECRET);
      const provider=JSON.parse(text) as Provider;
      expect(provider.kind).toBe('api');
      for (const member of ['plan_credits','key_credits','key_usage','key_usage_supported']) expect(provider.facts).not.toHaveProperty(member);
      await expect(facts.getByLabel(/Example secret/)).toHaveValue('');
      await page.goto('/resources/providers/' + walk.provider + '?tab=overview');
      const card=page.getByRole('region',{name:'Details from Acme example plan',exact:true});
      await expect(card.getByRole('meter',{name:'5 hours',exact:true})).toHaveAttribute('aria-valuenow','35');
      await expect(card.getByText('12.50 USD remaining',{exact:true})).toBeVisible();
      await expect(card.getByText(/7\.50 USD used of/)).toBeVisible();
      await expect(card.getByRole('button',{name:'Use a reset',exact:true})).toBeVisible();
      await page.goto('/resources/providers/' + walk.provider + '?tab=models');
      await expect(page.getByText('Models reported by the source',{exact:true})).toBeVisible();
      await expect(page.getByText('acme-small',{exact:true})).toBeVisible();
      await expect(page.getByText('acme-large',{exact:true})).toBeVisible();
      // Discovery can update offered metadata; it neither adds nor enables an allowlist entry.
      const afterResponse=await context.request.get('/api/v1/providers/' + walk.provider);
      expect(afterResponse.ok()).toBe(true);
      const after=await afterResponse.json() as Provider;
      expect(after.models.map((model) => [model.model_name,model.enabled]).sort()).toEqual(enabledBefore);
      expect(doubles).toEqual([]);
      expect(setupContexts).toContain(walk.provider);
    } finally {await context.close();}
  });
});
