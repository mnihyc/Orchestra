/** Own click path / 自有点击路径: Usage → record details → compare counted credits with the original vendor total. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { requestRow } from '../doubles/request-row';
import { timingFixture } from '../doubles/performance';
import { scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`reported cost comparison ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height:1000 });
    await page.emulateMedia({ colorScheme:theme, reducedMotion:'reduce' });
    await scenario();
    const sources = ['reported', 'provider', 'rate_card'] as const;
    const rows = sources.map((source,index) => requestRow({
      id:`00000000-0000-4000-8000-00000000010${index + 1}`, at:new Date().toISOString(),
      rate_source:source, credits:source === 'reported' ? '0.038800' : '3.000000', request_ref:source === 'reported' ? 'lease.close' : 'request-1',
      rate:{input_per_mtok:'3',cached_input_per_mtok:'3',output_per_mtok:'3'},
      timing:timingFixture({web_searches:{state:'told',value:1},
        native_prices:{web_search:{currency:'USD',terms:[{amount:'0.005',counter:'web_searches'},{amount:'0.0125',counter:'web_search_results'}]}},
        reported_cost:{state:'told',value:{amount:source === 'reported' ? '0.0194' : '0.0071000000000000001',unit:'USD'}}}),
    }));
    await page.route('**/api/v1/usage/events?*', async (route) => route.fulfill({json:{items:rows,next_cursor:null}}));
    await signIn(page);
    if(locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox',{name:/^(Language|语言)$/}).click();
      await page.getByRole('option',{name:'简体中文',exact:true}).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto('/usage?range=7d');
    const records = page.getByRole('table',{name:words.usage.records.title});
    for (const [index,source] of sources.entries()) {
      const row=records.getByRole('row').nth(index+1);
      await expect(row).not.toContainText('≈');
      const button=row.getByRole('button',{name:words.usage.records.aboutRecord});
      await button.focus(); await page.keyboard.press('Enter');
      const details=page.getByRole('dialog',{name:words.usage.records.aboutRecord});
      await expect(details).toContainText(words.usageRequest.vendorTotal);
      await expect(details).toContainText(source === 'reported' ? '0.0194 USD' : '0.0071000000000000001 USD');
      await expect(details).toContainText(words.usageRequest.countedCredits);
      await expect(details).toContainText(source === 'reported' ? '0.038800' : '3.000000');
      if(source === 'reported') {
        await expect(details).toContainText(words.usage.records.fromReported);
        // The adapter's request reference is not a word of the record: it is one Copy ID act in its row.
        await expect(details).not.toContainText('lease.close');
        await expect(details.getByRole('button',{name:words.common.copyId}).first()).toBeVisible();
      }
      if(source === 'rate_card') await expect(details).toContainText(words.usage.records.fromCard);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await info.attach(`reported-cost-${source}-${locale}-${theme}-${width}`,{body:await page.screenshot({fullPage:true}),contentType:'image/png'});
      await page.keyboard.press('Escape');
      await expect(details).not.toBeVisible();
    }
    // EN: Open Usage, keyboard-open the close subtotal and each explicit-rate record, compare the actual total and counted credits even when native price terms need an unreported result count, then close with Escape.
    // 中文：打开“用量”，用键盘打开关闭小计及各明确费率记录；即使原生价格项需要未报告的结果数量，也对比实际总费用和计入的额度，再按 Escape 关闭。
  });
}
