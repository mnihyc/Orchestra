/** Negative controls prove the rules fail for the owner's classes rather than merely taking screenshots. */
import { expect, test } from '@playwright/test';
import { inspectLayout, trackLayoutPointer, type LayoutInput } from './layout-rules-dom.ts';
import { auditHighlightedOptions, auditHoverIcons, auditPage, duplicateAccessibleNames, pressLayoutKey } from './layout-rules-helpers.ts';

const input: LayoutInput = { duplicateAllowList: [], rawValues: ['mock-large', 'on_request'] };

test('authored title and description values remain data beside a matching tab and document heading', async ({ page }) => {
  await page.setContent('<main><div role="tablist"><button id="doc-tab" role="tab" aria-selected="true" aria-controls="doc-panel">Rollout plan</button></div><article id="doc-panel" role="tabpanel" aria-labelledby="doc-tab"><textarea aria-label="Title" data-content-role="document">Rollout plan</textarea><textarea aria-label="Description" data-content-role="document">Rollout plan</textarea><div data-content-role="document"><h1>Rollout plan</h1></div></article></main>');
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'duplicate-text')).toEqual([]);
  expect(await duplicateAccessibleNames(page)).toEqual([]);
});

test('authored field values cannot exempt repeated interface captions or control names', async ({ page }) => {
  await page.setContent('<main><button role="tab">Rollout plan</button><span>Rollout plan</span><textarea aria-label="Title" data-content-role="document">First title</textarea><textarea aria-label="Title" data-content-role="document">Second title</textarea></main>');
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'duplicate-text').map((fault) => fault.text)).toEqual(['Rollout plan']);
  expect((await duplicateAccessibleNames(page)).map((fault) => fault.text)).toEqual(['Title']);
});

const layers = [
  { name: 'menu', open: '<div role="menu">', close: '</div>' },
  { name: 'popover', open: '<div class="floating-layer">', close: '</div>' },
  { name: 'dialog', open: '<section role="dialog" aria-label="Current prompt">', close: '</section>' },
  { name: 'sheet', open: '<aside role="dialog" aria-label="Current sheet" data-prompt-id="sheet">', close: '</aside>' },
] as const;
for (const layer of layers) {
  test('a transient ' + layer.name + ' is read separately from the page beneath it', async ({ page }) => {
    await page.setContent('<main><span>Shared caption</span><button aria-label="Shared command">Page act</button>' + layer.open
      + '<span>Shared caption</span><button aria-label="Shared command">Layer act</button>' + layer.close + '</main>');
    const result = await page.evaluate(inspectLayout, input);
    expect(result.faults.filter((fault) => ['duplicate-text', 'duplicate-action'].includes(fault.rule))).toEqual([]);
    expect(await duplicateAccessibleNames(page)).toEqual([]);
  });
  test('duplicate words still fail inside one ' + layer.name, async ({ page }) => {
    await page.setContent('<main><span>Repeated caption</span>' + layer.open + '<span>Repeated caption</span><strong>Repeated caption</strong>' + layer.close + '</main>');
    const faults = (await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'duplicate-text');
    expect(faults.map((fault) => fault.text)).toEqual(['Repeated caption']);
    expect(faults[0]?.selectors).toHaveLength(2);
  });
  test('duplicate controls still fail inside one ' + layer.name, async ({ page }) => {
    await page.setContent('<main><button aria-label="Repeated command">Page act</button>' + layer.open
      + '<button aria-label="Repeated command">First act</button><button aria-label="Repeated command">Second act</button>' + layer.close + '</main>');
    const faults = (await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'duplicate-action');
    expect(faults.map((fault) => fault.text)).toEqual(['Repeated command']);
    expect(faults[0]?.selectors).toHaveLength(2);
    const names = await duplicateAccessibleNames(page);
    expect(names.map((fault) => fault.text)).toEqual(['Repeated command']);
    expect(names[0]?.selectors).toHaveLength(2);
  });
}
test('an open nested layer is separate from its parent layer', async ({ page }) => {
  await page.setContent('<div role="dialog" aria-label="Parent"><span>Shared caption</span><button aria-label="Shared command">Parent act</button><div class="floating-layer"><span>Shared caption</span><button aria-label="Shared command">Nested act</button></div></div>');
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => ['duplicate-text', 'duplicate-action'].includes(fault.rule))).toEqual([]);
  expect(await duplicateAccessibleNames(page)).toEqual([]);
});
test('an open layer cannot excuse duplicate page words or controls', async ({ page }) => {
  await page.setContent('<header><span>Repeated caption</span><button aria-label="Repeated command">First act</button></header><main><span>Repeated caption</span><button aria-label="Repeated command">Second act</button><div role="menu"><span>Repeated caption</span><button aria-label="Repeated command">Menu act</button></div></main>');
  const faults = (await page.evaluate(inspectLayout, input)).faults;
  for (const rule of ['duplicate-text', 'duplicate-action']) {
    const matches = faults.filter((fault) => fault.rule === rule);
    expect(matches).toHaveLength(1);
    expect(matches[0]?.selectors).toHaveLength(2);
  }
});

const ownedTab = '<div role="tablist"><button id="page-tab" role="tab" aria-selected="true" aria-controls="page-panel">Overview</button></div><section id="page-panel" role="tabpanel" aria-labelledby="page-tab"><h1>Overview</h1><p>Current contents</p></section>';
test('the selected tab and its own primary panel heading both show their name', async ({ page }) => {
  await page.setContent('<main>' + ownedTab + '</main>');
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'duplicate-text')).toEqual([]);
  expect(result.allowances).toEqual(['Overview: selected tab and its own panel heading']);
});
for (const variant of ['third copy', 'inactive tab', 'wrong panel', 'missing reverse link', 'unrelated heading', 'duplicate controls'] as const) {
  test('native tab context still rejects ' + variant, async ({ page }) => {
    await page.setContent('<main>' + ownedTab + '</main>');
    await page.evaluate((variant) => {
      const panel = document.getElementById('page-panel')!, tab = document.getElementById('page-tab')!;
      if (variant === 'third copy') panel.insertAdjacentHTML('beforeend', '<strong>Overview</strong>');
      if (variant === 'inactive tab') tab.setAttribute('aria-selected', 'false');
      if (variant === 'wrong panel') tab.setAttribute('aria-controls', 'another-panel');
      if (variant === 'missing reverse link') panel.removeAttribute('aria-labelledby');
      if (variant === 'unrelated heading') panel.innerHTML = '<h1>Another page</h1><h2>Overview</h2>';
      if (variant === 'duplicate controls') panel.insertAdjacentHTML('beforeend', '<button>Inspect</button><button>Inspect</button>');
    }, variant);
    const faults = (await page.evaluate(inspectLayout, input)).faults;
    expect(faults.filter((fault) => fault.rule === 'duplicate-text').map((fault) => fault.text)).toContain(variant === 'duplicate controls' ? 'Inspect' : 'Overview');
    if (variant === 'duplicate controls') expect(faults.filter((fault) => fault.rule === 'duplicate-action').map((fault) => fault.text)).toContain('Inspect');
  });
}

test('the same person keeps the same name and target in distinct native table rows', async ({ page }) => {
  await page.setContent('<main><table><thead><tr><th>Event</th><th>Actor</th></tr></thead><tbody><tr><th scope="row">Place deleted</th><td><a href="/u/ada">Ada Lovelace</a></td></tr><tr><th scope="row">Document renamed</th><td><a href="/u/ada">Ada Lovelace</a></td></tr></tbody></table></main>');
  expect(await duplicateAccessibleNames(page)).toEqual([]);
});

for (const target of ['/u/ada', '/u/dora']) test('duplicate person controls still fail inside one native row: ' + target, async ({ page }) => {
  await page.setContent('<main><table><thead><tr><th>Event</th><th>Actor</th><th>Owner</th></tr></thead><tbody><tr><th scope="row">Place deleted</th><td><a href="/u/ada">Ada Lovelace</a></td><td><a href="' + target + '">Ada Lovelace</a></td></tr></tbody></table></main>');
  expect((await duplicateAccessibleNames(page)).map((fault) => fault.text)).toEqual(['Ada Lovelace']);
});

test('a tab owns its named panel region while duplicate controls inside that panel still fail', async ({ page }) => {
  await page.setContent('<style>.hidden-heading{position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0,0,0,0)}</style><main><div role="tablist" aria-label="Resource"><button role="tab" aria-selected="true">Overview</button></div><div role="tabpanel"><section role="region" aria-labelledby="panel-heading" style="display:contents"><h2 id="panel-heading" class="hidden-heading">Overview</h2><p>Current contents</p></section></div></main>');
  expect(await duplicateAccessibleNames(page)).toEqual([]);
  await page.locator('section').evaluate((section) => { section.insertAdjacentHTML('beforeend', '<button>Inspect</button><button>Inspect</button>'); });
  expect((await duplicateAccessibleNames(page)).map((fault) => fault.text)).toEqual(['Inspect']);
});

test('visibility summaries match direct ancestor checks across clipping axes and changed ancestors', async ({ page }) => {
  await page.setContent('<style>.probe{display:block;width:18px;overflow-wrap:anywhere;font:16px Arial}.window{position:relative;width:40px;height:40px;overflow:clip}.moved-x{position:absolute;left:80px;top:0}.moved-y{position:absolute;left:0;top:80px}.inner{position:absolute;left:80px;top:0;width:40px;height:40px;overflow:clip}.wide{position:absolute;left:-80px;top:0;width:300px}.zero{position:relative;width:0;height:0}.absolute{position:absolute;left:0;top:0}.clipped-label{position:absolute;width:1px;height:1px;clip:rect(0,0,0,0);overflow:hidden}</style><span class="probe" data-case="plain"></span><div style="width:45px"><span class="probe" style="display:inline" data-case="inline-fragments"></span></div><span class="probe" style="display:contents" data-case="contents"></span><span class="probe" style="width:0;height:0;overflow:visible" data-case="zero-box"></span><div hidden><span class="probe" data-case="hidden"></span></div><div inert><span class="probe" data-case="inert"></span></div><div data-opacity style="opacity:0"><span class="probe" data-case="transparent"></span></div><div data-visibility style="visibility:hidden"><span class="probe" style="visibility:visible" data-case="restored-child"></span></div><div class="window"><span class="probe moved-x" data-case="outside-x"></span></div><div class="window"><span class="probe moved-y" data-case="outside-y"></span></div><div class="window"><div class="inner"><span class="probe wide" data-case="disjoint-clips"></span></div></div><div class="zero"><span class="probe absolute" data-case="zero-parent"></span></div><div class="clipped-label"><span class="probe" data-case="accessible-only"></span></div>');
  await page.locator('[data-case]').evaluateAll((elements) => { elements.forEach((element, index) => { element.textContent = 'Probe' + String(index) + 'ExtraordinaryWord'.repeat(8); }); });
  let previous = 0;
  for (const changed of [false, true]) {
    // This direct walk is the independent visibility oracle: summaries must preserve every
    // individual ancestor comparison, including clips on different axes and zero-size parents.
    const expected = await page.locator('[data-case]').evaluateAll((elements) => elements.filter((element) => {
      if (element.closest('script,style,template,[hidden],[inert]') !== null || element.getClientRects().length === 0) return false;
      const own = element.getBoundingClientRect();
      if (own.width === 0 || own.height === 0) return false;
      for (let at: Element | null = element; at !== null; at = at.parentElement) {
        const style = getComputedStyle(at), box = at.getBoundingClientRect(), legacyClip = style.getPropertyValue('clip');
        if (style.display === 'none' || style.visibility === 'hidden' || Number(style.opacity || '1') === 0) return false;
        if (box.width <= 1 && box.height <= 1 && ((legacyClip !== 'auto' && legacyClip !== '') || (style.clipPath !== 'none' && style.clipPath !== ''))) return false;
        if (at !== element) {
          if (['hidden', 'clip', 'auto', 'scroll'].includes(style.overflowX) && (own.right <= box.left || own.left >= box.right)) return false;
          if (['hidden', 'clip', 'auto', 'scroll'].includes(style.overflowY) && (own.bottom <= box.top || own.top >= box.bottom)) return false;
        }
      }
      return true;
    }).map((element) => element.textContent).sort());
    const result = await page.evaluate(inspectLayout, { ...input, mode: 'choices' as const });
    expect(result.faults.filter((fault) => fault.rule === 'choice-word-width').map((fault) => fault.text).sort()).toEqual(expected);
    expect(expected.length).toBeGreaterThan(previous);
    previous = expected.length;
    if (!changed) await page.evaluate(() => {
      for (const element of Array.from(document.querySelectorAll<HTMLElement>('[hidden],[inert],[data-opacity],[data-visibility],.window,.inner,.clipped-label'))) {
        element.removeAttribute('hidden'); element.removeAttribute('inert');
        element.style.opacity = '1'; element.style.visibility = 'visible'; element.style.overflow = 'visible'; element.style.setProperty('clip', 'auto');
      }
    });
  }
});

test('hover visibility includes transparent resting actions and retakes changed ancestor visibility', async ({ page }) => {
  await page.setContent('<style>.row{display:flex;align-items:flex-start;gap:8px}.words{font:16px/24px Arial}button{opacity:0;transform:translateY(20px);border:0;padding:0;background:transparent;width:20px;height:20px}.row:hover button{opacity:1}svg{display:block;width:20px;height:20px}</style><div class="row"><span class="words">The actual first line</span><button aria-label="Resting action"><svg><circle cx="10" cy="10" r="3"/></svg></button></div>');
  const resting = await page.evaluate(inspectLayout, input);
  expect(resting.icons).toHaveLength(1);
  expect((await auditHoverIcons(page, resting.icons)).filter((fault) => fault.rule === 'icon-centre')).toHaveLength(1);
  await page.locator('.row').evaluate((element) => { element.style.visibility = 'hidden'; });
  const hidden = await page.evaluate(inspectLayout, input);
  expect(hidden.icons).toEqual([]);
  expect(hidden.faults.filter((fault) => fault.rule === 'icon-centre')).toEqual([]);
});

test('floating cards must fit both viewport axes and expose their long content by scrolling', async ({ page }) => {
  await page.setViewportSize({ width: 400, height: 300 });
  await page.setContent('<style>.floating-layer{position:fixed;width:180px;height:80px;background:white;color:black}.content{height:400px}.outside{left:300px;top:250px}.clipped{left:8px;top:8px;overflow:hidden}.good{left:200px;top:8px;overflow:auto}</style><div aria-label="Outside layer" class="floating-layer outside">Outside</div><div aria-label="Clipped layer" class="floating-layer clipped"><div class="content">Clipped content</div></div><div aria-label="Scrollable layer" class="floating-layer good"><div class="content">Scrollable content</div></div>');
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'floating-bounds').map((fault) => fault.text)).toEqual(['Outside layer']);
  expect(result.faults.filter((fault) => fault.rule === 'floating-scroll').map((fault) => fault.text)).toEqual(['Clipped layer']);
});

test('tree items are repeated rows while repeated headings outside them remain a finding', async ({ page }) => {
  await page.setContent('<div role="tree"><div role="treeitem"><span>Research</span></div></div><h2>Research</h2>');
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'duplicate-text')).toEqual([]);
  await page.locator('body').evaluate((body) => { const heading = document.createElement('h2'); heading.textContent = 'Research'; body.append(heading); });
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'duplicate-text').map((fault) => fault.text)).toEqual(['Research']);
});

test('a vertical rail and a separate footer do not borrow an inline label from another pane', async ({ page }) => {
  await page.setContent('<style>.board{display:flex;gap:20px}.column{display:flex;flex-direction:column;height:400px;justify-content:space-between}.row{display:flex;align-items:center}button{border:0;background:transparent;padding:4px}svg{width:16px;height:16px}</style><div class="board"><nav class="column"><span>Rail title</span><button aria-label="Rail action"><svg></svg></button></nav><main class="column"><span>Page title</span><div class="row"><button aria-label="Footer action"><svg></svg></button><textarea aria-label="Message"></textarea></div></main></div>');
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'icon-centre')).toEqual([]);
});

test('a composed field is measured on its actual visible frame and still fails a weak frame', async ({ page }) => {
  await page.setContent('<style>body{background:white;color:black}.frame{display:flex;padding:4px;border:1px solid black;background:white}.weak{border-color:white}textarea{border:0;background:white}</style><div data-control-boundary class="frame"><textarea aria-label="Strong frame"></textarea></div><div data-control-boundary class="frame weak"><textarea aria-label="Weak frame"></textarea></div>');
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'control-boundary').map((fault) => fault.text)).toEqual(['Weak frame']);
});

test('transient probes retain every finding of their complete rule without repeating unrelated page audits', async ({ page }) => {
  await page.setContent('<style>body{font:16px Arial;background:white;color:black}.row{display:flex;align-items:flex-start;gap:8px}.icon{margin-top:20px;padding:0;border:0;background:white}.icon svg{width:16px;height:16px}.broken{display:block;width:18px;padding:8px;box-sizing:border-box;overflow-wrap:anywhere;color:white;background:white}</style><div class="row"><button class="icon" aria-label="Offset action"><svg></svg></button><span>Aligned label</span></div><div role="listbox"><div role="option" aria-selected="true"><span class="broken">Environment</span></div></div>');
  const all = (await page.evaluate(inspectLayout, input)).faults;
  const comparable = (faults: typeof all) => faults.map(({ rule, text, detail }) => [rule, text, detail]).sort((left, right) => JSON.stringify(left).localeCompare(JSON.stringify(right)));
  for (const mode of ['choices', 'icons'] as const) {
    const rules = mode === 'icons' ? ['icon-centre'] : ['choice-word-width', 'choice-text-contrast'];
    const expected = all.filter((fault) => rules.includes(fault.rule));
    expect(expected.length, `${mode} has a deciding negative control`).toBeGreaterThan(0);
    const transient = await page.evaluate(inspectLayout, { ...input, mode });
    expect(comparable(transient.faults)).toEqual(comparable(expected));
  }
});

test('a choice must fit its longest real word, even when its flex item breaks the text into letters', async ({ page }) => {
  await page.setContent('<style>.row{display:flex;gap:8px;width:300px;font:16px Arial}.broken{width:18px;padding:8px;flex:0 0 auto;box-sizing:border-box;overflow-wrap:anywhere}.good{width:max-content;flex:0 0 auto;max-width:100%;overflow-wrap:break-word}</style><div class="row"><span class="broken" data-choice-name>Environment</span><span class="good" data-choice-name>Workspace view</span></div>');
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'choice-word-width').map((fault) => fault.text)).toEqual(['Environment']);
});

for (const dark of [false, true]) test(`chosen and highlighted words are measured on their actual ground in ${dark ? 'dark' : 'light'}`, async ({ page }) => {
  const surface = dark ? '#111111' : '#ffffff', ink = dark ? '#ffffff' : '#111111';
  await page.setContent(`<style>body{background:${surface};color:${ink}}.low{color:${surface};background:${surface}}.good{color:${ink};background:${surface}}</style><div role="listbox"><div role="option" aria-selected="true" class="low"><span data-choice-name>Unreadable chosen name</span></div><div role="option" data-highlighted class="low"><span data-choice-name>Unreadable highlight</span><small>Unreadable hint</small></div><div role="option" aria-selected="true" class="good"><span data-choice-name>Readable choice</span></div></div>`);
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'choice-text-contrast').map((fault) => fault.text)).toEqual(['Unreadable chosen name', 'Unreadable highlight', 'Unreadable hint']);
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`ordinary labels and table cells cannot collapse into letters (${locale}, ${theme})`, async ({ page }) => {
    const names: readonly [string, string, string] = locale === 'en' ? ['Environment', 'Description', 'Several complete words stay readable'] : ['环境管理', '说明内容', '完整的名称保持可读'];
    const surface = theme === 'dark' ? '#111' : '#fff', ink = theme === 'dark' ? '#fff' : '#111';
    await page.setContent(`<html lang="${locale}"><style>body{background:${surface};color:${ink};font:16px Arial}.narrow{display:block;box-sizing:border-box;width:18px;padding:8px;overflow-wrap:anywhere}.healthy{display:block;width:100px;overflow-wrap:break-word}.cut{display:block;width:24px;overflow:hidden;white-space:nowrap;text-overflow:ellipsis}</style><span class="narrow">${names[0]}</span><table><tr><td><span class="narrow">${names[1]}</span></td></tr></table><span class="healthy">${names[2]}</span><span class="cut">DeliberatelyEllipsizedOriginal</span></html>`);
    const result = await page.evaluate(inspectLayout, input);
    expect(result.faults.filter((fault) => fault.rule === 'choice-word-width').map((fault) => fault.text)).toEqual(names.slice(0, 2));
    expect(result.cuts.map((cut) => cut.text)).toContain('DeliberatelyEllipsizedOriginal');
  });

  test(`cmdk, checked and faded choices keep readable visible text (${locale}, ${theme})`, async ({ page }) => {
    const labels: readonly [string, string, string, string, string] = locale === 'en' ? ['Chosen name', 'Checked name', 'Faded choice', 'Visible name', 'Inverse name'] : ['已选名称', '已选单选项', '淡色选项', '可见名称', '反色名称'];
    const surface = theme === 'dark' ? '#111' : '#fff', ink = theme === 'dark' ? '#fff' : '#111';
    await page.setContent(`<style>body{background:${surface};color:${ink}}.low{background:${surface};color:${surface}}.good{background:${surface};color:${ink}}</style><div role="listbox"><div role="option" cmdk-item data-selected="true" aria-selected="false" class="low"><span>${labels[0]}</span></div><div role="radio" aria-checked="true" class="low"><span>${labels[1]}</span></div><div style="opacity:.2"><div role="option" aria-selected="true" class="good"><span>${labels[2]}</span></div></div><div role="option" aria-selected="true" aria-disabled="true" class="low"><span aria-hidden="true">${labels[3]}</span></div><div role="option" aria-selected="true" class="low"><span style="background:${ink};color:${surface}">${labels[4]}</span></div></div>`);
    const result = await page.evaluate(inspectLayout, input);
    expect(result.faults.filter((fault) => fault.rule === 'choice-text-contrast').map((fault) => fault.text)).toEqual(labels.slice(0, 4));
  });

  test(`the audit exposes pointer-only and keyboard-only highlights without activating them (${locale}, ${theme})`, async ({ page }) => {
    const labels: readonly [string, string, string] = locale === 'en' ? ['Readable option', 'Keyboard failure', 'Hover failure'] : ['可读选项', '键盘选项不可读', '悬停选项不可读'];
    const surface = theme === 'dark' ? '#111' : '#fff', ink = theme === 'dark' ? '#fff' : '#111';
    await page.setContent(`<style>body{background:${surface};color:${ink}}button{display:block;color:${ink};background:${surface}}#keyboard[data-selected="true"],#hover:hover{color:${surface}}</style><input role="combobox" aria-label="Choices" aria-controls="choices"><div id="choices" role="listbox"><button role="option" id="readable">${labels[0]}</button><button role="option" id="keyboard">${labels[1]}</button><button role="option" id="hover">${labels[2]}</button></div>`);
    await page.evaluate(() => {
      const field = document.querySelector('input');
      if (field === null) throw new Error('The choice input is missing.');
      const options = Array.from(document.querySelectorAll<HTMLElement>('[role="option"]'));
      let selected = 0;
      const choose = (next: number) => { selected = next; options.forEach((option, index) => { option.dataset.selected = String(index === selected); }); };
      field.addEventListener('keydown', (event) => {
        if (event.key === 'Home') { event.preventDefault(); choose(0); }
        if (event.key === 'ArrowDown') { event.preventDefault(); choose((selected + 1) % options.length); }
      });
      for (const [index, option] of options.entries()) {
        option.addEventListener('pointermove', () => choose(index));
        option.addEventListener('click', () => { document.documentElement.setAttribute('data-activated', 'true'); });
      }
      choose(0); field.focus();
    });
    const faults = await auditHighlightedOptions(page);
    expect([...new Set(faults.filter((fault) => fault.rule === 'choice-text-contrast').map((fault) => fault.text))]).toEqual(labels.slice(1));
    await expect(page.locator('html')).not.toHaveAttribute('data-activated');
  });

  test(`icon and text Star controls cannot evade the global duplicate rule (${locale}, ${theme})`, async ({ page }) => {
    const star = locale === 'en' ? 'Star' : '收藏';
    const surface = theme === 'dark' ? '#111' : '#fff', ink = theme === 'dark' ? '#fff' : '#111';
    await page.setContent(`<style>body{background:${surface};color:${ink}}</style><header><nav aria-label="Page actions"><button aria-label="${star}" aria-pressed="false"><svg aria-hidden="true" width="16" height="16"></svg></button></nav><section aria-label="Other actions"><button>${star}</button></section></header><ul><li><button aria-label="${star} first row">${star}</button></li><li><button aria-label="${star} second row">${star}</button></li></ul><button aria-label="${star} Acme">Acme</button><button aria-label="${star} Beta">Beta</button>`);
    // The old two independent rules each see one occurrence: no icon text and separate landmarks.
    expect(await duplicateAccessibleNames(page)).toEqual([]);
    const result = await page.evaluate(inspectLayout, input);
    expect(result.faults.filter((fault) => fault.rule === 'duplicate-text')).toEqual([]);
    const duplicates = result.faults.filter((fault) => fault.rule === 'duplicate-action');
    expect(duplicates).toHaveLength(1); expect(duplicates[0]?.text).toBe(star); expect(duplicates[0]?.selectors).toHaveLength(2);
  });
}

test('the chosen text threshold distinguishes 4.54:1 from 4.48:1', async ({ page }) => {
  await page.setContent('<style>body{background:white}.safe{color:#767676}.unsafe{color:#777777}</style><div role="listbox"><div role="option" aria-selected="true" class="safe">Readable threshold</div><div role="option" aria-selected="true" class="unsafe">Below threshold</div></div>');
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'choice-text-contrast').map((fault) => fault.text)).toEqual(['Below threshold']);
});

test('an image or gradient ground cannot silently pass as its transparent background colour', async ({ page }) => {
  await page.setContent('<style>body{background:black;color:white}.image{background-image:linear-gradient(white,white)}</style><div role="option" aria-selected="true" class="image">Image needs review</div>');
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'choice-text-contrast').map((fault) => fault.text)).toEqual(['Image needs review']);
});

test('visible duplicates cross landmarks, while list, table and timeline rows are exempt', async ({ page }) => {
  await page.setContent('<header><span data-user-content>Model</span></header><main><strong>Model</strong><ul><li>Repeated row</li><li>Repeated row</li></ul><table><tr><td>Table value</td></tr><tr><td>Table value</td></tr></table><div data-item="one">Timeline value</div><div data-item="two">Timeline value</div></main>');
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'duplicate-text').map((fault) => fault.text)).toEqual(['Model']);
});

test('an exact reasoned exception permits just its named occurrences, never a third or a blank reason', async ({ page }) => {
  await page.setContent('<span id="one">Allowed example</span><span id="two">Allowed example</span>');
  const allowance = { text: 'Allowed example', selectors: ['#one', '#two'], reason: 'This negative-control fixture deliberately repeats its example.' };
  const allowed = await page.evaluate(inspectLayout, { ...input, duplicateAllowList: [allowance] });
  expect(allowed.faults.filter((fault) => fault.rule === 'duplicate-text')).toEqual([]);
  expect(allowed.allowances).toHaveLength(1);
  const unexplained = await page.evaluate(inspectLayout, { ...input, duplicateAllowList: [{ ...allowance, reason: '' }] });
  expect(unexplained.faults.some((fault) => fault.rule === 'duplicate-text')).toBe(true);
  await page.locator('body').evaluate((body) => { const extra = document.createElement('span'); extra.textContent = 'Allowed example'; body.append(extra); });
  const third = await page.evaluate(inspectLayout, { ...input, duplicateAllowList: [allowance] });
  expect(third.faults.some((fault) => fault.rule === 'duplicate-text')).toBe(true);
});

test('duplicate accessible names use the computed name and the nearest landmark', async ({ page }) => {
  await page.setContent('<main><span id="named">Open details</span><button aria-labelledby="named">One</button><button aria-label="Open details">Two</button><h2><button>One nested name</button></h2></main><nav aria-label="Separate"><button aria-label="Open details">Other landmark</button></nav>');
  const result = await duplicateAccessibleNames(page);
  expect(result).toHaveLength(1);
  expect(result[0]?.text).toBe('Open details');
  expect(result[0]?.selectors).toHaveLength(2);
});

test('disclosing a clipped value requires ownership and cannot excuse another original', async ({ page }) => {
  await page.setContent('<style>.cut{display:block;width:40px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}</style><span class="cut" aria-describedby="whole">A complete value to disclose</span><div role="tooltip" id="whole">A complete value to disclose</div>');
  const given = { ...input, fullValueDisclosure: { reason: 'Reveal the original clipped value.' } };
  const allowed = await page.evaluate(inspectLayout, given);
  expect(allowed.faults.filter((fault) => fault.rule === 'duplicate-text')).toEqual([]);
  expect(allowed.allowances).toHaveLength(1);
  await page.locator('.cut').evaluate((element) => element.removeAttribute('aria-describedby'));
  expect((await page.evaluate(inspectLayout, given)).faults.some((fault) => fault.rule === 'duplicate-text')).toBe(true);
  await page.locator('.cut').evaluate((element) => { element.setAttribute('aria-describedby', 'whole'); element.removeAttribute('class'); });
  expect((await page.evaluate(inspectLayout, given)).faults.some((fault) => fault.rule === 'duplicate-text')).toBe(true);
  await page.locator('[aria-describedby="whole"]').evaluate((element) => {
    element.setAttribute('class', 'cut'); const duplicate = document.createElement('span'); duplicate.textContent = element.textContent; element.after(duplicate);
  });
  expect((await page.evaluate(inspectLayout, given)).faults.some((fault) => fault.rule === 'duplicate-text')).toBe(true);
});

test('toolbar More and inert ellipses fail, while a row action menu and a clickable disclosure remain distinct', async ({ page }) => {
  await page.setContent('<nav><button aria-haspopup="menu">More</button></nav><p>…</p><button aria-haspopup="menu" aria-label="More acts for the first turn">⋯</button><button aria-expanded="false">…</button>');
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'more-overflow')).toHaveLength(1);
  expect(result.faults.filter((fault) => fault.rule === 'inert-ellipsis')).toHaveLength(1);
});

test('empty naming attributes fall through and legacy clipped accessible labels stay out of visible duplicates', async ({ page }) => {
  await page.setContent('<main><span id="blank"></span><button aria-labelledby="blank" aria-label="" title="More" style="width:40px;height:32px"><svg aria-hidden="true" width="16" height="16"></svg></button><span style="position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0,0,0,0)">A single visible name</span><p>A single visible name</p></main>');
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'more-overflow').map((fault) => fault.text)).toEqual(['More']);
  expect(result.faults.filter((fault) => fault.rule === 'duplicate-text')).toEqual([]);
});

for (const dark of [false, true]) test(`control boundaries are actually measured in ${dark ? 'dark' : 'light'}`, async ({ page }) => {
  const surface = dark ? '#111111' : '#ffffff', strong = dark ? '#ffffff' : '#000000';
  await page.setContent(`<style>body{background:${surface}}input{background:${surface};border:1px solid ${surface};color:${strong}}#strong{border-color:${strong}}</style><input aria-label="Weak"><input id="strong" aria-label="Strong"><input disabled aria-label="Inactive">`);
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'control-boundary').map((fault) => fault.text)).toEqual(['Weak']);
});

test('descriptions belong in the tooltip or an open picker, and routing words stay out of ordinary copy', async ({ page }) => {
  await page.setContent('<span data-choice-description>Not on the page</span><div role="tooltip"><span data-choice-description>In the tooltip</span></div><div role="menu"><span data-choice-description>In the open menu</span></div><p>settings.updated</p><p>mock-large</p><code>package.version</code><input value="on_request"><div data-user-content>config.json and on_request are literal user data</div>');
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'choice-description')).toHaveLength(1);
  expect(result.faults.filter((fault) => fault.rule === 'raw-name').map((fault) => fault.text)).toEqual(['settings.updated', 'mock-large']);
});

test('a costly selected choice cannot bring back an in-page caption or description area', async ({ page }) => {
  await page.setContent('<p data-choice-selected-description>Only you can use it; others can be named later.</p><div data-choice-descriptions>Every option’s explanation under the control.</div><div role="tooltip"><p data-choice-selected-description>On demand in a tooltip.</p></div><div role="listbox"><p data-choice-description>Inside the open picker.</p></div>');
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults.filter((fault) => fault.rule === 'choice-description').map((fault) => fault.text)).toEqual([
    'Only you can use it; others can be named later.', 'Every option’s explanation under the control.',
  ]);
});

test('actual ellipsis and a line clamp require a keyboard path to the whole value', async ({ page }) => {
  await page.setContent('<style>.cut{display:block;width:30px;overflow:hidden;white-space:nowrap;text-overflow:ellipsis}.clamp{display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:1;width:40px;overflow:hidden}</style><span class="cut">The whole cut value</span><span class="clamp">The complete clamped value on several lines</span><a href="/"><span class="cut">A focusable full value</span></a>');
  const result = await page.evaluate(inspectLayout, input);
  expect(result.cuts).toHaveLength(3);
  expect(result.faults.filter((fault) => fault.rule === 'cut-text')).toHaveLength(2);
});

test('the icon threshold is one pixel from the first text line, rather than the whole multi-line row', async ({ page }) => {
  await page.setContent('<style>.row{display:flex;align-items:flex-start;gap:8px}.words{font:16px/24px Arial;width:100px}button{border:0;padding:0;background:transparent;width:20px;height:20px;flex:none}svg{display:block;width:20px;height:20px}</style><div class="row"><span class="words">The first line and then another line</span><button aria-label="Row actions"><svg viewBox="0 0 20 20"><circle cx="10" cy="10" r="3"/></svg></button></div>');
  const origin = await page.locator('.row').evaluate((row) => {
    const text = row.querySelector('.words')?.firstChild;
    const button = row.querySelector('button');
    if (text === undefined || text === null || button === null) throw new Error('Missing fixture line.');
    const range = document.createRange(); range.selectNodeContents(text);
    const line = range.getClientRects()[0];
    if (line === undefined) throw new Error('No measured first line.');
    const box = button.getBoundingClientRect();
    return line.top + line.height / 2 - box.top - box.height / 2;
  });
  await page.locator('button').evaluate((button, shift) => { button.style.transform = `translateY(${shift + 0.5}px)`; }, origin);
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'icon-centre')).toEqual([]);
  await page.locator('button').evaluate((button, shift) => { button.style.transform = `translateY(${shift + 1}px)`; }, origin);
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'icon-centre')).toHaveLength(1);
});


for (const kind of ['key', 'pointer'] as const) test(`a pipelined ${kind} measurement waits for its real trusted input`, async ({ page }) => {
  await page.setContent('<button id="input-target">Target</button>');
  await page.evaluate(trackLayoutPointer);
  let finished = false;
  const pending = page.evaluate(inspectLayout, { duplicateAllowList: [], rawValues: [], mode: 'icons' as const, settle: true,
    ...(kind === 'pointer' ? { pointerTarget: '#input-target' } : {}), afterInput: { kind, sequence: 0 } });
  void pending.then(() => { finished = true; }, () => { finished = true; });
  await page.evaluate(() => new Promise<void>((done) => requestAnimationFrame(() => requestAnimationFrame(() => done()))));
  expect(finished, 'two frames without the required input cannot complete the measurement').toBe(false);
  if (kind === 'key') await page.keyboard.press('Home'); else await page.locator('#input-target').hover();
  expect((await pending).faults).toEqual([]);
});


test('noneditable controls inside an editor retain the actual first-line alignment check', async ({ page }) => {
  await page.setContent('<style>.row{display:flex;align-items:center;gap:8px}.words{font:16px/28px Arial}button{border:0;padding:0;background:transparent;width:28px;height:28px;display:flex;align-items:center;justify-content:center}svg{width:16px;height:16px}</style><div contenteditable="true"><p>Another document block</p><div class="row" contenteditable="false"><span class="words">Language</span><button aria-label="Copy code"><svg viewBox="0 0 16 16"><circle cx="8" cy="8" r="3"/></svg></button></div></div>');
  const origin = await page.locator('.row').evaluate((row) => {
    const text = row.querySelector('.words')?.firstChild, icon = row.querySelector('svg');
    if (text === undefined || text === null || icon === null) throw new Error('Missing native toolbar label');
    const range = document.createRange(); range.selectNodeContents(text);
    const line = range.getClientRects()[0], box = icon.getBoundingClientRect();
    if (line === undefined) throw new Error('The actual label has no first line');
    return line.top + line.height / 2 - box.top - box.height / 2;
  });
  await page.locator('button').evaluate((button, offset) => { button.style.transform = `translateY(${offset + 0.5}px)`; }, origin);
  expect((await page.evaluate(inspectLayout, { ...input, mode: 'icons' as const })).faults).toEqual([]);
  await page.locator('button').evaluate((button, offset) => { button.style.transform = `translateY(${offset + 1}px)`; }, origin);
  expect((await page.evaluate(inspectLayout, { ...input, mode: 'icons' as const })).faults.map((fault) => fault.text)).toEqual(['Copy code']);
});

test('verbatim source words are data while interface captions remain checked', async ({ page }) => {
  await page.setContent('<main><h1>Model</h1><pre data-content-role="source"><h2>Model</h2><span>---</span><span>---</span></pre></main>');
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'duplicate-text')).toEqual([]);
  expect(await duplicateAccessibleNames(page)).toEqual([]);
  await page.locator('main').evaluate((node) => { const caption = document.createElement('span'); caption.textContent = 'Model'; node.append(caption); });
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'duplicate-text').map((fault) => fault.text)).toEqual(['Model']);
});

test('authored document words and headings compare within each actual block', async ({ page }) => {
  await page.setContent('<main><h1>Rollout plan</h1><div data-content-role="document" contenteditable="true" role="textbox" aria-label="Document body"><div data-ob="block"><h2>Rollout plan</h2></div><div data-ob="block"><h2>Rollout plan</h2></div></div></main>');
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'duplicate-text')).toEqual([]);
  expect(await duplicateAccessibleNames(page)).toEqual([]);
  await page.locator('[data-ob="block"]').first().evaluate((block) => { const heading = document.createElement('h3'); heading.textContent = 'Rollout plan'; block.append(heading); });
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'duplicate-text').map((fault) => fault.text)).toEqual(['Rollout plan']);
  expect((await duplicateAccessibleNames(page)).map((fault) => fault.text)).toEqual(['Rollout plan']);
});

for (const role of ['source', 'document']) test('duplicate controls remain checked inside ' + role + ' content', async ({ page }) => {
  await page.setContent(`<main><div data-content-role="${role}" contenteditable="false"><div data-ob="block"><button>Inspect</button><button>Inspect</button></div></div></main>`);
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'duplicate-text').map((fault) => fault.text)).toEqual(['Inspect']);
  expect((await duplicateAccessibleNames(page)).map((fault) => fault.text)).toEqual(['Inspect']);
});

test('noneditable interface words inside authored document content are not exempt', async ({ page }) => {
  await page.setContent('<main><span>Language</span><div data-content-role="document" contenteditable="true"><div data-ob="block"><p>Authored words</p><div contenteditable="false"><span>Language</span></div></div></div></main>');
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'duplicate-text').map((fault) => fault.text)).toEqual(['Language']);
});


test('native text and overflow reads are fresh after the next actual input', async ({ page }) => {
  await page.setContent('<button id="change">Change</button><span id="value" tabindex="0" style="display:block;width:20px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis">First complete long value</span>');
  const before = await page.evaluate(inspectLayout, input);
  expect(before.cuts.map((cut) => cut.text)).toContain('First complete long value');
  await page.evaluate(() => document.getElementById('change')?.addEventListener('keydown', (event) => {
    if (event.key === 'Home') { const value = document.getElementById('value'); if (value !== null) value.textContent = 'Second complete long value'; }
  }));
  await page.locator('#change').focus(); await page.keyboard.press('Home');
  const after = await page.evaluate(inspectLayout, input);
  expect(after.cuts.map((cut) => cut.text)).toContain('Second complete long value');
  expect(after.cuts.map((cut) => cut.text)).not.toContain('First complete long value');
});


for (const key of ['Home', 'ArrowDown', 'Escape'] as const) test('the layout key path preserves trusted native behavior for ' + key, async ({ page }) => {
  await page.setContent('<select size="3" aria-label="Native choices"><option>First</option><option>Second</option><option>Third</option></select><dialog><button>Native dialog action</button></dialog>');
  await page.evaluate((key) => {
    const select = document.querySelector('select');
    if (select === null) throw new Error('Missing native select');
    select.selectedIndex = key === 'Home' ? 2 : 0;
    select.focus();
    if (key === 'Escape') document.querySelector('dialog')?.showModal();
    const state = globalThis as typeof globalThis & { nativeKeyTrace?: { type: string; key: string; trusted: boolean }[] };
    state.nativeKeyTrace = [];
    document.addEventListener('keydown', (event) => state.nativeKeyTrace?.push({ type: event.type, key: event.key, trusted: event.isTrusted }), true);
    document.addEventListener('keyup', (event) => state.nativeKeyTrace?.push({ type: event.type, key: event.key, trusted: event.isTrusted }), true);
  }, key);
  await pressLayoutKey(page, key);
  expect(await page.evaluate(() => (globalThis as typeof globalThis & { nativeKeyTrace?: unknown[] }).nativeKeyTrace)).toEqual([
    { type: 'keydown', key, trusted: true }, { type: 'keyup', key, trusted: true },
  ]);
  if (key === 'Escape') await expect(page.getByRole('dialog')).toBeHidden();
  else await expect(page.getByRole('listbox')).toHaveValue(key === 'Home' ? 'First' : 'Second');
});


test('transient checks retain findings while full passes own target discovery', async ({ page }) => {
  await page.setContent('<style>body{background:white;color:black}.bad{color:white;background:white}</style><button aria-label="Menu" aria-haspopup="menu"><svg width="16" height="16"></svg></button><div role="listbox"><button role="option" aria-selected="true" class="bad">Unreadable selected words</button></div>');
  const full = await page.evaluate(inspectLayout, input);
  expect(full.menus).toHaveLength(1);
  expect(full.icons).toHaveLength(1);
  const transient = await page.evaluate(inspectLayout, { ...input, mode: 'choices' as const });
  expect(transient.faults.filter((fault) => fault.rule === 'choice-text-contrast')).toEqual(full.faults.filter((fault) => fault.rule === 'choice-text-contrast'));
  expect(transient.faults.some((fault) => fault.text === 'Unreadable selected words')).toBe(true);
  expect(transient.menus).toEqual([]); expect(transient.icons).toEqual([]);
});


test('browser-live style handles retake changed colour, opacity, width and ancestor state', async ({ page }) => {
  await page.setContent('<style>body{background:white;color:black}.words{display:block;width:300px;font:16px/24px Arial}.changed{opacity:.2}.changed .words{width:18px;color:white}</style><button id="change">Change</button><div id="parent"><div role="option" aria-selected="true" class="words">CompleteSelectedWords</div></div>');
  const before = await page.evaluate(inspectLayout, { ...input, mode: 'choices' as const });
  expect(before.faults).toEqual([]);
  await page.evaluate(() => document.getElementById('change')?.addEventListener('keydown', (event) => {
    if (event.key === 'Home') document.getElementById('parent')?.classList.add('changed');
  }));
  await page.locator('#change').focus(); await pressLayoutKey(page, 'Home');
  const direct = await page.locator('.words').evaluate((element) => {
    const style = getComputedStyle(element); return { width: style.width, colour: style.color, opacity: getComputedStyle(element.parentElement!).opacity };
  });
  expect(direct).toEqual({ width: '18px', colour: 'rgb(255, 255, 255)', opacity: '0.2' });
  const changed = await page.evaluate(inspectLayout, { ...input, mode: 'choices' as const });
  expect(changed.faults.map((fault) => fault.rule)).toEqual(expect.arrayContaining(['choice-word-width', 'choice-text-contrast']));
  await page.locator('#parent').evaluate((element) => element.classList.remove('changed'));
  expect((await page.evaluate(inspectLayout, { ...input, mode: 'choices' as const })).faults).toEqual([]);
});


test('cached native font names still retake changed font sizes and letter spacing', async ({ page }) => {
  await page.setContent('<span id="words" style="display:block;width:200px;font:16px Arial">Environment</span>');
  expect((await page.evaluate(inspectLayout, { ...input, mode: 'choices' as const })).faults).toEqual([]);
  await page.locator('#words').evaluate((element) => { element.style.fontSize = '64px'; });
  expect((await page.evaluate(inspectLayout, { ...input, mode: 'choices' as const })).faults.some((fault) => fault.rule === 'choice-word-width')).toBe(true);
  await page.locator('#words').evaluate((element) => { element.style.fontSize = '16px'; element.style.letterSpacing = '20px'; });
  expect((await page.evaluate(inspectLayout, { ...input, mode: 'choices' as const })).faults.some((fault) => fault.rule === 'choice-word-width')).toBe(true);
  await page.locator('#words').evaluate((element) => { element.style.letterSpacing = 'normal'; });
  expect((await page.evaluate(inspectLayout, { ...input, mode: 'choices' as const })).faults).toEqual([]);
});


test('menu walks reuse executable code while actual input measurements stay fresh', async ({ page }) => {
  await page.setContent('<button role="option" aria-selected="true">First option</button><button role="option" aria-selected="false">Second option</button>');
  await auditHighlightedOptions(page);
  await page.evaluate(() => { const state = globalThis as typeof globalThis & { __orchLayoutInspect?: unknown; previousInspector?: unknown }; state.previousInspector = state.__orchLayoutInspect; });
  await auditHighlightedOptions(page);
  expect(await page.evaluate(() => { const state = globalThis as typeof globalThis & { __orchLayoutInspect?: unknown; previousInspector?: unknown }; return state.__orchLayoutInspect === state.previousInspector; })).toBe(true);
  await page.setContent('<button role="option" aria-selected="true">New document option</button>');
  expect(await auditHighlightedOptions(page)).toEqual([]);
});

test('closed native disclosures do not expose their hidden fields as pointer targets', async ({ page }, info) => {
  await page.setContent('<style>body{background:white;color:black}</style><details><summary>More fields</summary><button id="inside" aria-label="Hidden actions" aria-haspopup="menu" style="width:100px;height:28px;border:1px solid black;background:white"><svg width="16" height="16"></svg></button><div role="option" aria-selected="true" style="color:white;background:white">Unreadable hidden choice</div></details>');
  const native = await page.locator('#inside').evaluate((element) => {
    const box = element.getBoundingClientRect();
    return { width: box.width, height: box.height, rects: element.getClientRects().length, visible: element.checkVisibility() };
  });
  await info.attach('native-disclosure', { body: JSON.stringify(native), contentType: 'application/json' });
  expect(native.visible).toBe(false);
  const result = await page.evaluate(inspectLayout, input);
  expect(result.faults).toEqual([]);
  expect(result.icons).toEqual([]);
  expect(result.menus).toEqual([]);
});

test('native disclosure summaries retain their ordinary duplicate text check', async ({ page }) => {
  await page.setContent('<h1>More fields</h1><p>Secondary summary</p><details><summary>More fields</summary>Secondary summary<summary>Secondary summary</summary><div>More fields</div></details>');
  expect(await page.locator('summary').first().evaluate((element) => element.checkVisibility())).toBe(true);
  expect(await page.locator('summary').nth(1).evaluate((element) => element.checkVisibility())).toBe(false);
  const duplicates = (await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'duplicate-text');
  expect(duplicates.map((fault) => fault.text)).toEqual(['More fields']);
  expect(duplicates[0]?.selectors).toHaveLength(2);
});

test('opening a native disclosure checks its fields and closing it retakes visibility', async ({ page }) => {
  await page.setContent('<style>body{background:white;color:black}</style><details><summary>More fields</summary><button aria-label="Hidden actions" aria-haspopup="menu" style="border:1px solid black;background:white"><svg width="16" height="16"></svg></button><div role="option" aria-selected="true" style="color:white;background:white">Unreadable hidden choice</div></details>');
  await page.locator('summary').focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('details')).toHaveJSProperty('open', true);
  const opened = await page.evaluate(inspectLayout, { ...input, settle: true });
  expect(opened.faults.some((fault) => fault.rule === 'choice-text-contrast' && fault.text === 'Unreadable hidden choice')).toBe(true);
  expect(opened.icons).toHaveLength(1);
  expect(opened.menus).toHaveLength(1);
  await page.locator('summary').focus();
  await page.keyboard.press('Enter');
  await expect(page.locator('details')).toHaveJSProperty('open', false);
  const closed = await page.evaluate(inspectLayout, { ...input, settle: true });
  expect(closed.faults).toEqual([]);
  expect(closed.icons).toEqual([]);
  expect(closed.menus).toEqual([]);
});

for (const position of ['absolute', 'fixed']) test('a positioned toolbar keeps its own first-line comparison: ' + position, async ({ page }) => {
  await page.setContent(`<style>body{margin:0;background:white;color:black}main{position:relative;display:flex;height:600px}.outside{margin-top:300px;font:16px/24px Arial}.frame{position:${position};top:0}.row{display:flex;align-items:center;gap:8px;font:16px/24px Arial}button{border:0;padding:0;background:transparent;width:20px;height:20px}svg{display:block;width:20px;height:20px}</style><main><span class="outside">Outside words</span><div class="frame" style="left:0"><div class="row"><button aria-label="Floating action"><svg></svg></button></div></div><div class="frame row" style="left:200px"><span>Inside words</span><button id="labelled" aria-label="Labelled action"><svg></svg></button></div></main>`);
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'icon-centre')).toEqual([]);
  await page.locator('#labelled').evaluate((element) => { element.style.transform = 'translateY(4px)'; });
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'icon-centre').map((fault) => fault.text)).toEqual(['Labelled action']);
});

test('an editor control surface keeps local labels without borrowing another surface', async ({ page }) => {
  await page.setContent('<style>body{margin:0;background:white;color:black}main{display:flex;height:600px}.outside{margin-top:300px;font:16px/24px Arial}.row{display:flex;align-items:center;gap:8px;font:16px/24px Arial}button{border:0;padding:0;background:transparent;width:20px;height:20px}svg{display:block;width:20px;height:20px}</style><main contenteditable="true"><span class="outside" contenteditable="false">Outside words</span><div contenteditable="false"><div class="row"><button aria-label="Unlabelled surface action"><svg></svg></button></div></div><div class="row" contenteditable="false"><span>Inside words</span><button id="labelled" aria-label="Local surface action"><svg></svg></button></div></main>');
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'icon-centre')).toEqual([]);
  await page.locator('#labelled').evaluate((element) => { element.style.transform = 'translateY(4px)'; });
  expect((await page.evaluate(inspectLayout, input)).faults.filter((fault) => fault.rule === 'icon-centre').map((fault) => fault.text)).toEqual(['Local surface action']);
});

for (const mode of ['complete', 'unowned hint', 'stolen focus'] as const) test(`full-value disclosure checks native focus and its own hint: ${mode}`, async ({ page }) => {
  const value = 'A complete value that is wider than this small control';
  await page.setContent('<style>body{margin:0;background:white;color:black}.cut{display:block;width:80px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}</style><main><span id="owner" class="cut" tabindex="0"></span><span id="other" tabindex="-1"></span></main>');
  await page.locator('#owner').evaluate((owner, setup) => {
    owner.textContent = setup.value;
    const show = () => {
      let hint = document.getElementById('whole-value');
      if (hint === null) { hint = document.createElement('div'); hint.id = 'whole-value'; hint.setAttribute('role', 'tooltip'); document.body.append(hint); }
      hint.textContent = setup.value;
      if (setup.mode !== 'unowned hint') owner.setAttribute('aria-describedby', hint.id);
    };
    owner.addEventListener('pointerover', show);
    owner.addEventListener('focus', () => { show(); if (setup.mode === 'stolen focus') document.getElementById('other')?.focus(); });
    document.addEventListener('keydown', (event) => { if (event.key === 'Escape') { document.getElementById('whole-value')?.remove(); owner.removeAttribute('aria-describedby'); } });
  }, { value, mode });
  const faults = (await auditPage(page)).faults.filter((fault) => fault.rule === 'cut-text');
  expect(faults.map((fault) => fault.text)).toEqual(mode === 'complete' ? [] : [value]);
  if (mode === 'complete') await expect(page.locator('#owner')).toBeFocused();
});
