/** Wizard actions use the same control tokens as ordinary dialogs, without stretching to their panels. */
import { expect, type Locator } from '@playwright/test';

export async function expectOrdinaryWizardControls(dialog: Locator, stage: string): Promise<void> {
  const measured=await dialog.evaluate((root) => {
    const issues:string[]=[];
    // Resolve the actual token (including appearance scale), rather than assuming a pixel height.
    const probe=document.createElement('div');
    probe.style.cssText='position:fixed;visibility:hidden;width:0;height:var(--control-md)';
    document.body.append(probe);
    const ordinary=probe.getBoundingClientRect().height;
    probe.remove();
    const bounds=root.getBoundingClientRect();
    // One CSS pixel permits only fractional-layout rounding, not additional control padding.
    const rounding=1;
    for(const caption of Array.from(root.querySelectorAll('[data-choice-descriptions],[data-choice-selected-description],[data-choice-description],[data-choice-help]'))) {
      const rect=caption.getBoundingClientRect();
      if(rect.height>1 && rect.width>1 && getComputedStyle(caption).visibility==='visible') issues.push('an in-page option has a description block or per-option guide');
    }
    for(const element of Array.from(root.querySelectorAll('button,a[data-action]'))) {
      if(!(element instanceof HTMLElement)) continue;
      const rect=element.getBoundingClientRect();
      if(rect.width===0 || rect.height===0) continue;
      const style=getComputedStyle(element);
      if(style.visibility!=='visible') continue;
      const name=element.getAttribute('aria-label') ?? element.textContent;
      if(rect.height>ordinary+rounding) issues.push(`${name}: height ${rect.height}, ordinary ${ordinary}`);
      if(rect.left<bounds.left-rounding || rect.right>bounds.right+rounding) issues.push(`${name}: outside its dialog`);
      const role=element.getAttribute('role');
      // Form fields and selector options have their own shared layout contract. Check action widths.
      const action=element.closest('footer')!==null || element.hasAttribute('data-action') || element.hasAttribute('data-change-choice');
      if(!action || ['combobox','radio','checkbox','switch','tab'].includes(role ?? '') || element.innerText.trim()==='') continue;
      const clone=element.cloneNode(true) as HTMLElement;
      clone.removeAttribute('id');
      for(const identified of Array.from(clone.querySelectorAll('[id]'))) identified.removeAttribute('id');
      clone.setAttribute('aria-hidden','true');
      clone.setAttribute('tabindex','-1');
      clone.style.cssText='position:fixed;left:0;top:0;visibility:hidden;width:max-content;min-width:0;max-width:none;flex:none;align-self:flex-start';
      clone.style.font=style.font;
      clone.style.letterSpacing=style.letterSpacing;
      document.body.append(clone);
      const intrinsic=clone.getBoundingClientRect().width;
      clone.remove();
      if(rect.width>intrinsic+rounding) issues.push(`${name}: stretched ${rect.width}, content ${intrinsic}`);
    }
    return {issues,ordinary,width:bounds.width,viewport:innerWidth};
  });
  expect(measured.ordinary,`${stage}: ordinary dialog control token`).toBeGreaterThan(0);
  expect(measured.width,`${stage}: dialog fits viewport`).toBeLessThanOrEqual(measured.viewport);
  expect(measured.issues,stage).toEqual([]);
}

/** A reason gets its own row; two ordinary actions that fit at 400px stay beside one another. */
export async function expectSameActionRow(actions: Locator): Promise<void> {
  const buttons=await actions.getByRole('button').evaluateAll((elements) => elements.map((element) => {
    const rect=element.getBoundingClientRect();
    return {top:rect.top,bottom:rect.bottom};
  }));
  expect(buttons).toHaveLength(2);
  const cancel=buttons[0];
  const create=buttons[1];
  if(cancel===undefined || create===undefined) throw new Error('Cancel and Create must both be present');
  expect(Math.abs(cancel.top-create.top)).toBeLessThanOrEqual(1);
  expect(Math.abs(cancel.bottom-create.bottom)).toBeLessThanOrEqual(1);
}
