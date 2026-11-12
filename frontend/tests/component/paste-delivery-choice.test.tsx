/** A sender changes one folded paste's delivery without changing its bytes or another paste. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it } from 'vitest';
import { AttachmentChips } from '@/features/composer/Attachments';
import type { PastedTextPart } from '@/lib/pastedText';
import { show } from './support';
import { i18n, setLocale } from '@/i18n';

const paste:PastedTextPart={type:'pasted_text',blob:'a'.repeat(64),name:'paste.txt',path:'.orch/attachments/paste.txt',size:4000,characters:4000,ascii_characters:4000};
afterEach(async()=>{cleanup();await setLocale('en');});
function Draft(props: { estimated?: boolean | undefined }){const estimated = 'estimated' in props ? props.estimated : false;const [parts,setParts]=useState([paste,{...paste,name:'other.txt',path:'.orch/attachments/other.txt'}]);return <AttachmentChips parts={parts} pasteMode={()=>estimated} destination={(part)=>'path' in part ? `→ ${part.path}` : null} onPasteDelivery={(part,delivery)=>setParts((before)=>before.map((held)=>held===part?{...held,delivery}:held))}/>;}
it('switches between message and file for that paste alone and exposes the current choice',async()=>{
 const user=userEvent.setup();show(<Draft/>);
 const controls=screen.getAllByRole('radiogroup',{name:'Pasted text delivery'});
 expect(controls).toHaveLength(2);
 await user.click(within(controls[0]!).getByRole('radio',{name:'In the message'}));
 expect(within(controls[0]!).getByRole('radio',{name:'In the message'})).toBeChecked();expect(within(controls[1]!).getByRole('radio',{name:'As a file'})).toBeChecked();
 await user.click(within(controls[0]!).getByRole('radio',{name:'As a file'}));
 expect(within(controls[0]!).getByRole('radio',{name:'As a file'})).toBeChecked();
 expect(within(screen.getByRole('list',{name:i18n.t('uploads.ready')})).getAllByText('4,000 characters')).toHaveLength(2);
});

it.each(['en', 'zh-CN'] as const)('leaves unknown paste delivery unselected but accepts an explicit choice in %s', async (locale) => {
 await setLocale(locale); show(<Draft estimated={undefined} />);
 const group = screen.getAllByRole('radiogroup', { name: i18n.t('uploads.pasteDelivery') })[0];
 if (group === undefined) throw new Error('The paste needs its existing delivery chooser');
 for (const option of within(group).getAllByRole('radio')) expect(option).not.toBeChecked();
 expect(screen.queryByText(i18n.t('uploads.pasteSuggested'))).not.toBeInTheDocument();
 const item = group.closest('li');
 if (item === null) throw new Error('The chooser belongs to its pasted text chip');
 await userEvent.click(within(group).getByRole('radio', { name: i18n.t('uploads.inMessage') }));
 expect(within(group).getByRole('radio', { name: i18n.t('uploads.inMessage') })).toBeChecked();
 expect(within(item).queryByText(`→ ${paste.path}`, { exact: true })).not.toBeInTheDocument();
 expect(within(item).getByText(i18n.t('uploads.pasteInline'), { exact: true })).toBeVisible();
});
