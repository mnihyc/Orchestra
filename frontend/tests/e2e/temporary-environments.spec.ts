/** A new operator makes temporary work at an explicit place, keeps it or exports from the removal notice. */
import { expect, test } from '@playwright/test';
import { bodySent, ACME, scenario, signIn } from './support.ts';
import { id, KIND } from '../doubles/fixtures.ts';
import { requestsOf, setDefaults } from './start-support.ts';

const DOUBLE=`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
async function warning(environment:string):Promise<void> {
  const response=await fetch(`${DOUBLE}/__double/resources/temporary-warning`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({id:environment})});
  if(!response.ok) throw new Error(`the warning was refused: ${response.status}`);
}
test.beforeEach(async()=>{await scenario();});

for(const width of [1440,400]) {
  test(`makes a temporary environment from its template in the viewed place and keeps it (${width}px)`,async({page})=>{
    await page.setViewportSize({width,height:900});await signIn(page);
    await page.goto(`/resources/templates/${id(KIND.template,1)}`);
    await page.getByRole('button',{name:'Add environment',exact:true}).click();
    const dialog=page.getByRole('dialog',{name:'Add environment'});
    await expect(dialog.getByRole('combobox',{name:'Belongs to'})).toContainText('Acme');
    await expect(dialog.getByRole('combobox',{name:'Template',exact:true})).toContainText('Node');
    await dialog.getByRole('textbox',{name:'Name',exact:true}).fill(`Disposable ${width}`);
    await dialog.getByRole('switch',{name:'Temporary',exact:true}).click();
    await expect(dialog.getByText('Removes its owned files after a one-hour warning.')).toBeVisible();
    const created=page.waitForRequest(request=>request.method()==='POST'&&new URL(request.url()).pathname==='/api/v1/environments');
    await dialog.getByRole('button',{name:'Add environment',exact:true}).click();
    expect((await bodySent(await created))).toMatchObject({home_node_id:ACME.org,template_id:id(KIND.template,1),config:{'env.lifetime':'temporary'}});
    await expect(page.getByRole('heading',{name:`Disposable ${width}`,level:1})).toBeVisible();
    await expect(page.locator('[data-temporary-environment]:visible')).toContainText('Temporary · removed');
    const kept=page.waitForResponse(response=>response.request().method()==='POST'&&new URL(response.url()).pathname.endsWith('/keep'));
    await page.locator('[data-temporary-environment]:visible').getByRole('button',{name:'Keep',exact:true}).click();
    expect((await kept).status()).toBe(200);
    await expect(page.locator('[data-temporary-environment]:visible')).toHaveCount(0);
    await page.reload();await expect(page.getByRole('heading',{name:`Disposable ${width}`,level:1})).toBeVisible();
    await expect(page.locator('[data-temporary-environment]:visible')).toHaveCount(0);
  });
}

test('the host entry presets that host and the owner can export directly from the warning notice',async({page})=>{
  await signIn(page);await page.goto(`/resources/hosts/${id(KIND.host,1)}`);
  await page.getByRole('button',{name:'Add environment',exact:true}).click();
  const dialog=page.getByRole('dialog',{name:'Add environment'});
  await expect(dialog.getByRole('combobox',{name:'Host',exact:true})).toContainText('host-1');
  await dialog.getByRole('textbox',{name:'Name',exact:true}).fill('Export experiment');
  await dialog.getByRole('switch',{name:'Temporary',exact:true}).click();
  const created=page.waitForResponse(response=>response.request().method()==='POST'&&new URL(response.url()).pathname==='/api/v1/environments');
  await dialog.getByRole('button',{name:'Add environment',exact:true}).click();
  const reply:unknown=await (await created).json();
  expect(reply).toMatchObject({host:{id:id(KIND.host,1)}});
  await expect(page.getByRole('heading',{name:'Export experiment',level:1})).toBeVisible();
  const match=/\/resources\/environments\/([^/?]+)/.exec(page.url());
  if(match?.[1]===undefined) throw new Error('the environment did not open');
  await warning(match[1]);await page.goto('/inbox');
  const notice=page.locator('li[data-kind="environment.attention"]').filter({hasText:'Export experiment will be removed'});
  await expect(notice).toContainText('owned files');
  await expect(notice.getByRole('button',{name:'Keep',exact:true})).toBeVisible();
  await notice.getByRole('button',{name:'Export files',exact:true}).click();
  const exportDialog=page.getByRole('dialog',{name:'Export workspace files'});
  const download=page.waitForEvent('download');
  await exportDialog.getByRole('button',{name:'Download ZIP',exact:true}).click();
  expect((await download).suggestedFilename()).toBe('Export experiment.zip');
  await exportDialog.getByRole('button',{name:'Close',exact:true}).last().click();
  await expect(page.locator('li[data-kind="job.finished"]').getByRole('button',{name:'Download ZIP',exact:true})).toBeVisible();
});

test('a refused place is focused with its error visible inside the creation window',async({page})=>{
  await page.setViewportSize({width:400,height:700});await signIn(page);await page.goto('/resources/environments');
  await page.getByRole('button',{name:'Add environment',exact:true}).click();
  await page.getByRole('menuitem',{name:/Make from a template/}).click();
  const dialog=page.getByRole('dialog',{name:'Add environment'});
  await expect(dialog.getByRole('combobox',{name:'Belongs to'})).toContainText('Acme');
  await page.route('**/api/v1/environments',async(route)=>{
    if(route.request().method()!=='POST'){await route.continue();return;}
    await route.fulfill({status:400,contentType:'application/problem+json',body:JSON.stringify({type:'orch:error:request.invalid',code:'request.invalid',title:'Choose a place',status:400,detail:'Choose a place',request_id:'place-refusal',errors:[{path:'home_node_id',message:'Choose a place you may create environments in.'}]})});
  });
  await dialog.getByRole('button',{name:'Add environment',exact:true}).click();
  const belongs=dialog.getByRole('combobox',{name:'Belongs to'});
  await expect(belongs).toBeFocused();await expect(belongs).toHaveAttribute('aria-invalid','true');
  await expect(belongs).toHaveAccessibleDescription('Choose a place you may create environments in.');
  await expect(dialog.getByText('Choose a place you may create environments in.')).toBeInViewport();
});

test('a new thread can choose disposable work without changing the topics lasting default',async({page})=>{
  await setDefaults(ACME.fixLogin,{'env.thread_default':'fresh'});
  await signIn(page);await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByRole('button',{name:'New thread',exact:true}).first().click();
  const dialog=page.getByRole('dialog',{name:'New thread in Fix login'});
  await dialog.getByRole('switch',{name:'Temporary',exact:true}).click();
  await dialog.getByRole('button',{name:'Create thread',exact:true}).click();
  await expect(page).toHaveURL(/\/thread\//);
  const made=await requestsOf('POST',/\/environments$/);
  expect(made).toHaveLength(1);
  expect(made[0]?.body).toMatchObject({home_node_id:ACME.fixLogin,config:{'env.lifetime':'thread'}});
  expect(await requestsOf('PUT',/\/default-environment$/)).toHaveLength(0);
});
