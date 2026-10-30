/** The same mark and action are used in a machine row, a node row and their page notices. */
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe,expect,it,vi } from 'vitest';
import type { ComponentUpdate } from '@/generated/api-types';
import { UpdateMark,UpdateNotice } from '@/features/resources/ComponentUpdate';
import { show } from './support';

// Guide navigation is covered by its own router tests; this fixture exercises the shared update controls.
vi.mock('@/features/help/ArticleGuideLink',()=>({ArticleGuideLink:()=>null}));

const behind:ComponentUpdate = {running_version:'1.2.0',recommended_version:'1.3.0',behind:true,status:'current',detail:null,command:'orch-node update'};

describe('physical component updates',()=>{
  it('marks a behind row and removes the mark only when the reported version catches up',()=>{
    const view=show(<UpdateMark update={behind} />);
    expect(screen.getByText('Update available')).toBeInTheDocument();
    view.rerender(<UpdateMark update={{...behind,status:'queued'}} />);
    expect(screen.getByRole('status')).toHaveTextContent('Updating…');
    view.rerender(<UpdateMark update={{...behind,running_version:'1.3.0',behind:false,status:'current'}} />);
    expect(screen.queryByText('Update available')).not.toBeInTheDocument();
    expect(screen.queryByText('Updating…')).not.toBeInTheDocument();
  });

  it('names both versions and invokes the shared Update now action',async()=>{
    const run=vi.fn();
    show(<UpdateNotice update={behind} action={{id:'update-now',label:'Update now',disabled:false,run}} />);
    expect(screen.getByText('Update available: v1.3.0 (this machine runs v1.2.0)')).toBeInTheDocument();
    await userEvent.setup().click(screen.getByRole('button',{name:'Update now'}));
    expect(run).toHaveBeenCalledOnce();
  });

  it('keeps the local key and command visible when the machine refuses',()=>{
    const refused={...behind,status:'refused',detail:'update.auto is off on this machine. Run `orch-node update` on the machine.'};
    show(<UpdateNotice update={refused} action={null} />);
    // The command's copy button has its own status region for clipboard feedback.
    const reason=screen.getByText(refused.detail);
    expect(reason).toBeVisible();
    expect(reason).toHaveAttribute('role','status');
    expect(reason).toHaveTextContent('update.auto');
    expect(reason).toHaveTextContent('orch-node update');
    expect(screen.queryByText('Updating…')).not.toBeInTheDocument();
    expect(screen.getByText('orch-node update',{selector:'code'})).toBeInTheDocument();
  });

  it('keeps a pending action disabled while the row waits for its new-version reconnect',()=>{
    show(<UpdateNotice update={{...behind,status:'updating'}} action={{id:'update-now',label:'Updating…',disabled:'Updating…',pending:true,run:vi.fn()}} />);
    expect(screen.getByRole('button',{name:'Updating…'})).toBeDisabled();
  });
});
