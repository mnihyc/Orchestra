/** A sender changes one folded paste's delivery without changing its bytes or another paste. */
import { cleanup, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it } from 'vitest';
import { AttachmentChips } from '@/features/composer/Attachments';
import type { PastedTextPart } from '@/lib/pastedText';
import { show } from './support';
import { i18n } from '@/i18n';

const paste:PastedTextPart={type:'pasted_text',blob:'a'.repeat(64),name:'paste.txt',path:'.orch/attachments/paste.txt',size:4000,characters:4000,ascii_characters:4000};
afterEach(cleanup);
function Draft(){const [parts,setParts]=useState([paste,{...paste,name:'other.txt',path:'.orch/attachments/other.txt'}]);return <AttachmentChips parts={parts} pasteMode={()=>false} onPasteDelivery={(part,delivery)=>setParts((before)=>before.map((held)=>held===part?{...held,delivery}:held))}/>;}
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
