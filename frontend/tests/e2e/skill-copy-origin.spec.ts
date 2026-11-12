/** Native skill_duplicates.rs decides copy authority and bytes; this walk checks the delivered copy origin on real screens. */
import { expect, test } from '@playwright/test';
import { DOUBLE, scenario, signIn } from './support';
import { chats } from './chat-support';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };

for (const locale of ['en','zh-CN'] as const) for (const width of [1440,400]) for (const theme of ['light','dark'] as const) {
  test(`a copied skill has a readable origin through its library and version clicks in ${locale}/${width}/${theme}`, async ({page},info) => {
    await scenario(); await chats('/locale',{as:'ada',locale});
    await page.setViewportSize({width,height:900}); await page.emulateMedia({colorScheme:theme});
    // The canonical producer supplies the copied origin to inventory, live detail and every version read.
    expect((await page.request.post(`${DOUBLE}/__double/request-feeds/fixtures`, { data: { skill: { origin: { kind: 'copy' } } } })).status()).toBe(204);
    await signIn(page);
    await page.goto('/resources/skills');
    await page.getByRole('link',{name:'pdf-tools',exact:true}).first().click();
    await expect(page.getByRole('heading',{name:'pdf-tools',exact:true})).toBeVisible();
    const words=locale==='en'?english:chinese;
    const label=locale==='en'?'Copied in Orch':'在 Orch 中复制';
    await expect(page.getByText(label,{exact:true}).first()).toBeVisible();
    await expect(page.getByText('https://git.example.test/acme/skills.git',{exact:true})).toHaveCount(0);
    await page.getByRole('tab',{name:words.resourceDetails.skill.versions,exact:true}).click();
    await expect(page.getByText(label,{exact:true}).first()).toBeVisible();
    await page.getByRole('tab',{name:words.resourceDetails.overview,exact:true}).click();
    await expect(page.getByText(label,{exact:true}).first()).toBeVisible();
    await expect.poll(()=>page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth)).toBe(true);
    await info.attach(`skill-copy-${locale}-${width}-${theme}`,{body:await page.screenshot({fullPage:true}),contentType:'image/png'});
  });
}
