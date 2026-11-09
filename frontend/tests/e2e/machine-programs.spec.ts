/** The fake world only supplies the machine connection; enrollment and both program workflows use the screens. */
import { expect,test } from '@playwright/test';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { PEOPLE,scenario,signIn } from './support.ts';
const DOUBLE=`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
test.beforeEach(async()=>{await scenario();});

test('explicit enrollment opens one Programs workflow for agent adapters and provider sources',async({page})=>{
  await signIn(page,PEOPLE.ada);await page.goto('/resources/machines');
  let issued=0;page.on('request',(request)=>{if(request.method()==='POST' && request.url().endsWith('/machines/enrollment-tokens'))issued+=1;});
  await page.getByRole('button',{name:'Add machine'}).click();
  const enrollment=page.getByRole('dialog',{name:'Add machine'});
  await enrollment.getByRole('checkbox',{name:'Provider',exact:true}).click();
  for(const role of ['Environment','Agent','Provider'])await enrollment.getByRole('group',{name:role,exact:true}).getByRole('switch').click();
  expect(issued).toBe(0);
  await enrollment.getByRole('button',{name:'Show command'}).click();
  const code=enrollment.locator('pre code');await expect(code).toContainText('oenr_');expect(issued).toBe(1);
  await enrollment.getByRole('tab',{name:'Linux',exact:true}).click();
  const token=/--token '([^']+)'/u.exec(await code.innerText())?.[1];
  if(token===undefined)throw new Error('No token in the displayed command');
  const connected=await fetch(`${DOUBLE}/__double/resources/enroll`,{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify({token,name:'Programs machine'})});
  expect(connected.ok).toBeTruthy();
  await expect(enrollment).toHaveCount(0);
  await expect(page.getByRole('heading',{name:'Programs machine',level:1,exact:true})).toBeVisible();
  await page.getByRole('tab',{name:/^Registered/u}).click();
  const sections=page.getByRole('tabpanel',{name:'Registered here',exact:true}).getByRole('navigation',{name:'Registered here',exact:true}).filter({has:page.getByRole('link',{name:'Programs',exact:true,includeHidden:true})});
  await sections.getByRole('combobox',{name:'Registered here',exact:true}).click();
  await page.getByRole('option',{name:'Programs',exact:true}).click();
  for(const [contract,name,command] of [['Agent adapter','Local agent','/opt/local-agent --stdio'],['Provider source','GPU source','/opt/gpu-source --device 0']] as const){
    await page.getByRole('combobox',{name:'Contract',exact:true}).click();
    await page.getByRole('option',{name:new RegExp(contract,'u')}).click();
    await page.getByRole('button',{name:'Add',exact:true}).click();
    const dialog=page.getByRole('dialog',{name:'Register a program'});
    await dialog.getByRole('textbox',{name:'Name',exact:true}).fill(name);
    await dialog.getByRole('textbox',{name:'Command',exact:true}).fill(command);
    await dialog.getByRole('button',{name:'Register and describe'}).click();await expect(dialog).toHaveCount(0);
    const row=page.getByRole('list',{name:'Programs',exact:true}).getByRole('listitem').filter({hasText:name});
    await expect(row).toContainText('1.0.0');
    if(contract==='Provider source')await expect(row).toContainText('Local model');
    await row.getByRole('button',{name:'Test',exact:true}).click();
    if(contract==='Provider source')await expect(row).toContainText('Local model');
    await row.getByRole('button',{name:'Delete',exact:true}).click();
    await page.getByRole('alertdialog').getByRole('button',{name:'Delete',exact:true}).click();await expect(row).toHaveCount(0);
  }
});

test('an environment read failure says what could not be read and Retry recovers its stored view',async({page})=>{
  await signIn(page,PEOPLE.ada);
  const target=new RegExp(`/api/v1/environments/${RUNS_ON.workspace}/sync(?:\\?.*)?$`,'u');
  const record=(value:unknown):value is Record<string,unknown>=>typeof value==='object' && value!==null && !Array.isArray(value);
  let unavailable=true;
  // A valid native row correctly heals an HTTP-only failure; this outage owns both projections.
  await page.routeWebSocket(/\/api\/v1\/ws(?:\?|$)/u,(socket)=>{
    const server=socket.connectToServer();
    server.onMessage((raw)=>{
      const frame:unknown=JSON.parse(String(raw));
      if(unavailable && record(frame) && (
        frame.op==='sync' && frame.feed===`env:${RUNS_ON.workspace}` ||
        frame.op==='event' && frame.feed===`resource:environment:${RUNS_ON.workspace}` ||
        frame.op==='event' && frame.feed==='user' && record(frame.data) && frame.data.what==='resources' && frame.data.kind==='environment' && frame.data.id===RUNS_ON.workspace
      ))return;
      socket.send(raw);
    });
  });
  await page.route(target,(route)=>route.fulfill({status:503,contentType:'application/problem+json',json:{type:'orch:error:source.offline',code:'source.offline',title:'Read failed',status:503,detail:'Stored report unavailable',request_id:'stored-read'}}));
  const failed=page.waitForResponse(target);
  await page.goto(`/resources/environments/${RUNS_ON.workspace}`);
  expect((await failed).status()).toBe(503);
  await expect(page.getByRole('alert')).toContainText('Could not read this environment');
  await page.unroute(target);const retried=page.waitForResponse(target);await page.getByRole('button',{name:'Try again',exact:true}).click();
  expect((await retried).status()).toBe(200);unavailable=false;
  await expect(page.getByRole('heading',{name:'Workspace',level:1,exact:true})).toBeVisible();
});

test('a kept machine explains its roles with its own instance command and sends no role mutation',async({page})=>{
  await signIn(page,PEOPLE.ada);let writes=0;
  page.on('request',(request)=>{if(request.method()==='PUT' && request.url().endsWith('/roles'))writes+=1;});
  await page.goto(`/resources/machines/${RUNS_ON.laptop}`);
  await expect(page.getByText('Roles are kept on this machine.',{exact:true})).toBeVisible();
  await expect(page.locator('code').filter({hasText:/orch-connector.*--data-dir.*roles set/u})).toHaveCount(1);
  await expect(page.getByRole('button',{name:'Change roles',exact:true})).toHaveCount(0);
  expect(writes).toBe(0);
});
