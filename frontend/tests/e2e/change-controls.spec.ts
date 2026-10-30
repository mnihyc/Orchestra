/** Ordinary wizard pickers and action rows at desktop and phone widths in both themes. */
import { expect, type Locator, type Page, test } from '@playwright/test';

import { RUNS_ON } from '../doubles/fixtures.ts';
import { expectOrdinaryWizardControls, expectSameActionRow } from './change-control-geometry.ts';
import { BUILD_HOST, change, conversation, openBlocked, openHistory, tabTo, wizard } from './change-support.ts';
import { startDialog } from './start-support.ts';
import { ACME, scenario, signIn } from './support.ts';

async function picker(page:Page,dialog:Locator,name:string):Promise<void> {
  const choice=dialog.getByRole('combobox',{name,exact:true});
  await expect(choice).toBeVisible();
  await tabTo(page,name);
  await expect(choice).toBeFocused();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('listbox')).toBeVisible();
  await page.keyboard.press('Escape');
  await expect(choice).toBeFocused();
  await expectOrdinaryWizardControls(dialog,`${name} picker`);
}

for(const width of [1440,400]) for(const theme of ['light','dark'] as const) {
  test.describe(`ordinary wizard controls at ${width}px in ${theme}`,() => {
    test.beforeEach(async ({page}) => {
      await page.setViewportSize({width,height:width===400?860:900});
      await page.emulateMedia({colorScheme:theme});
      await scenario();
      await change.mode('manual');
    });

    test('a switch, its pickers, pending confirmation, progress and done',async ({page}) => {
      await openBlocked(page);
      await page.getByRole('button',{name:'Switch provider…'}).click();
      const dialog=wizard(page,'Switch “Main thread”');
      await expect(dialog.locator('[data-plan]')).toBeVisible();
      await expect(page.locator('html')).toHaveAttribute('data-theme',theme);
      await expect(dialog.getByRole('list',{name:'Steps',exact:true})).toHaveCount(0);
      await expect(dialog.getByText('What it runs on',{exact:true})).toHaveCount(0);
      await expect(dialog.locator('[data-thread-change-pickers]').getByRole('combobox',{name:'Environment',exact:true})).toHaveCount(0);
      await expectOrdinaryWizardControls(dialog,'switch plan');
      await picker(page,dialog,'Agent and provider');
      await expect(dialog.getByRole('button',{name:/^Options/})).toHaveCount(0);
      await expectOrdinaryWizardControls(dialog,'compact switch confirmation');

      let release:()=>void=()=>undefined;
      const held=new Promise<void>(resolve=>{release=resolve;});
      await page.route('**/api/v1/threads/*/forks',async route=>{await held;await route.continue();},{times:1});
      try {
        await expect(dialog.locator('[data-confirm]')).not.toHaveAttribute('aria-disabled','true');
        await dialog.getByRole('button',{name:'Switch',exact:true}).click();
        await expect(dialog.locator('[data-change-view]')).toHaveAttribute('data-change-view','confirm');
        await expectOrdinaryWizardControls(dialog,'pending confirmation');
      } finally {release();}
      await expect(dialog.locator('[data-change-view]')).toHaveAttribute('data-change-view','progress');
      await expectOrdinaryWizardControls(dialog,'switch progress');
      await change.finish();
      await expect(dialog.locator('[data-change-view]')).toHaveAttribute('data-change-view','done');
      await expectOrdinaryWizardControls(dialog,'switch done');
    });

    test('fork history, runtime, environment, options and the resulting thread action',async ({page}) => {
      await openHistory(page);
      await conversation(page).getByText('The handler reads').first().click({button:'right'});
      await page.getByRole('menuitem',{name:'Fork from here…'}).click();
      const dialog=wizard(page,'Fork “Main thread”');
      await expect(dialog.locator('[data-plan]')).toBeVisible();
      await expect(page.locator('html')).toHaveAttribute('data-theme',theme);
      for(const name of ['History','Provider','Environment']) await picker(page,dialog,name);
      await dialog.getByRole('button',{name:/^Options/}).click();
      await expectOrdinaryWizardControls(dialog,'fork options');
      await expect(dialog.locator('[data-confirm]')).not.toHaveAttribute('aria-disabled','true');
      await dialog.getByRole('button',{name:'Fork',exact:true}).click();
      await expect(dialog.locator('[data-change-view]')).toHaveAttribute('data-change-view','progress');
      await expectOrdinaryWizardControls(dialog,'fork progress');
      await change.finish();
      await expect(dialog.getByRole('button',{name:'Open the new thread'})).toBeVisible();
      await expectOrdinaryWizardControls(dialog,'fork done');
    });

    test('environment update/recreate steps and a blocked move retry do not stretch controls',async ({page}) => {
      await signIn(page);
      await page.goto(`/resources/environments/${RUNS_ON.sandbox}`);
      await page.getByRole('button',{name:'Update…'}).click();
      let dialog=wizard(page,'Update Sandbox');
      await expect(dialog.locator('[data-plan]')).toBeVisible();
      await expect(page.locator('html')).toHaveAttribute('data-theme',theme);
      await expectOrdinaryWizardControls(dialog,'environment update plan');
      await expect(dialog.locator('[data-confirm]')).not.toHaveAttribute('aria-disabled','true');
      await dialog.getByRole('button',{name:'Update',exact:true}).click();
      await expect(dialog.locator('[data-change-view]')).toHaveAttribute('data-change-view','progress');
      await expectOrdinaryWizardControls(dialog,'environment progress');
      await change.finish();
      await expect(dialog.getByRole('button',{name:'Done',exact:true})).toBeVisible();
      await expectOrdinaryWizardControls(dialog,'environment done');
      await dialog.getByRole('button',{name:'Done',exact:true}).click();

      await page.getByRole('button',{name:'Actions for Sandbox'}).click();
      await page.getByRole('menuitem',{name:'Recreate…',exact:true}).click();
      dialog=wizard(page,'Recreate Sandbox');
      await expect(dialog.locator('[data-plan]')).toBeVisible();
      await expectOrdinaryWizardControls(dialog,'recreate plan');
      await dialog.getByRole('button',{name:'Cancel',exact:true}).click();

      // A valid blocked-plan reply exposes the standalone retry control as well as the footer.
      await change.host({id:BUILD_HOST,schedulable:true});
      await page.route('**/api/v1/environments/*/moves/preflight',async route=>{
        const response=await route.fetch();
        const plan:unknown=await response.json();
        if(typeof plan!=='object' || plan===null || Array.isArray(plan)) throw new Error('a move plan must be an object');
        await route.fulfill({response,json:{...plan,blockers:[{code:'no_host',message:'No host is available.'}]}});
      });
      await page.getByRole('button',{name:'Actions for Sandbox'}).click();
      await page.getByRole('menuitem',{name:'Move…',exact:true}).click();
      dialog=wizard(page,'Move Sandbox');
      await expect(dialog.locator('[data-plan]').getByRole('button',{name:'Try again'})).toBeVisible();
      await expectOrdinaryWizardControls(dialog,'blocked move and retry');
      await picker(page,dialog,'Place');
    });

    test('a loading reason leaves Cancel and Create thread together',async ({page}) => {
      await signIn(page);
      let release:()=>void=()=>undefined;
      const held=new Promise<void>(resolve=>{release=resolve;});
      await page.route('**/api/v1/nodes/*/binding/options*',async route=>{await held;await route.continue();});
      try {
        await page.goto(`/topic/${ACME.fixLogin}`);
        await page.getByRole('button',{name:'New thread',exact:true}).first().click();
        const dialog=startDialog(page);
        await expect(dialog.getByText('Looking up what can be chosen…',{exact:true})).toBeVisible();
        await expect(page.locator('html')).toHaveAttribute('data-theme',theme);
        await expect(dialog.getByRole('button',{name:'Create thread',exact:true})).toBeDisabled();
        await expectSameActionRow(dialog.getByRole('group',{name:'Thread actions',exact:true}));
        await expectOrdinaryWizardControls(dialog,'new thread loading actions');
        await dialog.getByRole('button',{name:'Cancel',exact:true}).click();
      } finally {release();}
    });
  });
}
