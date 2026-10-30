/** Compare the library's added reads with the same resource frame in a separately served reference build. */
import { writeFile } from 'node:fs/promises';
import { expect, test, type Page } from '@playwright/test';
import { inventoryRow } from '../doubles/everything-row';
import { resources, facts } from './library-fixtures';
import { scenario, signIn } from './support';

const reference = process.env.ORCH_LIBRARY_REFERENCE_URL;
const inventory = (path: string) => path.startsWith('GET /api/v1/everything?') && path.includes('resource_details=true');
const signature = (path: string) => inventory(path) ? path : path.split('?')[0] ?? path;
function added(current: string[], frame: string[]): string[] {
  const remaining = frame.map(signature);
  return current.filter((request) => {
    const at = remaining.indexOf(signature(request));
    if (at < 0) return true;
    remaining.splice(at, 1); return false;
  });
}
async function records(page: Page) {
  await page.route('**/api/v1/everything?*',async(route)=>{
    const url=new URL(route.request().url());
    const resource=Object.values(resources).find((r)=>r.kind===url.searchParams.get('kinds'));
    if(url.searchParams.get('resource_details')!=='true'||resource===undefined){await route.continue();return;}
    await route.fulfill({json:{items:[inventoryRow({id:resource.value.id,kind:resource.kind,name:resource.value.name,my_access:'manage',resource,library:facts(resource)})],next_cursor:null}});
  });
}

for(const width of [1440,400])test(`each list adds one inventory read to its reference frame at ${width}px`,async({browser,baseURL},info)=>{
  test.skip(reference===undefined,'Requires a separately served reference build for the request difference');
  if(reference===undefined||baseURL===undefined)throw new Error('Both built applications must be served');
  const measured:Record<string,Record<string,string[]>>={};
  for(const [name,origin]of [['reference',reference],['current',baseURL]] as const){
    await scenario();
    const context=await browser.newContext({baseURL:origin,viewport:{width,height:900}});
    try {
      const page=await context.newPage();await records(page);await signIn(page);
      const requests:string[]=[];page.on('request',(request)=>{if(request.url().includes('/api/v1/'))requests.push(`${request.method()} ${new URL(request.url()).pathname}${new URL(request.url()).search}`);});
      const pages:Record<string,string[]>={};
      for(const route of [...Object.keys(resources),'providers']){
        const key=pages[route]===undefined?route:`${route}-warm`,before=requests.length;
        const served=page.waitForResponse((response)=>response.request().method()==='GET'&&inventory(`GET ${new URL(response.url()).pathname}${new URL(response.url()).search}`));
        await page.goto(`/resources/${route}`);await served;await page.waitForLoadState('networkidle');
        pages[key]=requests.slice(before);
      }
      measured[name]=pages;
    }finally{await context.close();}
  }
  const capture=info.outputPath('frame-request-difference.json');await writeFile(capture,JSON.stringify(measured,null,2));
  await info.attach('frame-request-difference',{path:capture,contentType:'application/json'});
  for(const [route,current]of Object.entries(measured.current ?? {})){
    const previous=measured.reference?.[route];expect(previous,route).toBeDefined();
    if(previous===undefined)throw new Error('The reference frame must have the same route');
    expect(previous.filter(inventory),`${route} reference`).toHaveLength(1);
    const frame=previous.filter((path)=>!inventory(path));
    const difference=added(current,frame);
    expect(current.filter(inventory),route).toHaveLength(1);
    expect(difference.filter(inventory),route).toHaveLength(1);
    expect(difference.filter((path)=>!inventory(path)),`${route}:\n${difference.join('\n')}`).toEqual([]);
    expect(current.filter((path)=>path.startsWith('POST /api/v1/resources/library')),route).toEqual([]);
  }
});
