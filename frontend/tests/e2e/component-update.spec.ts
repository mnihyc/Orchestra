/** A real screen/list/cache/live-feed walk, with explicit machine reconnect in the fake system. */
import { expect,test } from '@playwright/test';
import { scenario,signIn } from './support';

const DOUBLE=`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;

test('a behind machine shows Update available, Update now stays Updating until its new-version reconnect',async({page,request})=>{
  await scenario();
  const seeded=await request.post(`${DOUBLE}/__double/resources/component-update`,{data:{machine:'ada-laptop',running:'0.1.0',recommended:'0.2.0'}});
  expect(seeded.ok()).toBeTruthy();
  const {id}=await seeded.json() as {id:string};
  await signIn(page);
  await page.goto('/resources/machines');
  const row=page.getByRole('row').filter({has:page.getByRole('link',{name:'ada-laptop',exact:true})});
  await expect(row.getByText('Update available',{exact:true})).toBeVisible();
  await row.getByRole('link',{name:'ada-laptop',exact:true}).click();
  await expect(page.getByText('Update available: v0.2.0 (this machine runs v0.1.0)',{exact:true})).toBeVisible();
  await page.getByRole('button',{name:'Update now',exact:true}).first().click();
  await expect(page.getByText('Updating…',{exact:true}).first()).toBeVisible();
  await page.goto('/resources/machines');
  await expect(row.getByText('Updating…',{exact:true})).toBeVisible();
  const reconnected=await request.post(`${DOUBLE}/__double/resources/component-update`,{data:{machine:id,reconnect:true}});
  expect(reconnected.ok()).toBeTruthy();
  await expect(row.getByText('Updating…',{exact:true})).toHaveCount(0);
  await expect(row.getByText('Update available',{exact:true})).toHaveCount(0);
  await row.getByRole('link',{name:'ada-laptop',exact:true}).click();
  await expect(page.getByText('Update available: v0.2.0 (this machine runs v0.1.0)',{exact:true})).toHaveCount(0);
  await expect(page.getByRole('button',{name:'Update now',exact:true})).toHaveCount(0);
});
