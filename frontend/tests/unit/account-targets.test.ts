// @vitest-environment node
/** Accepted invitations use only returned readable references, including a guest chat without an org read. */
import { describe, expect, it } from 'vitest';
import { acceptedDestination } from '../../src/features/auth/accepted.ts';
import type { InvitationAcceptReply, NodeRef } from '../../src/generated/api-types.ts';
import { me, org, userRef } from '../doubles/fixtures.ts';

const empty: InvitationAcceptReply = {org:null,node:null,chat:null};
const node = (kind:NodeRef['kind']): NodeRef => ({kind,id:'returned',name:'Visible',redacted:false});

describe('the returned invitation target',() => {
  it('opens every readable node kind by its real route',() => {
    expect(acceptedDestination({...empty,node:node('org')})).toEqual({to:'/org/$slug',params:{slug:'returned'}});
    expect(acceptedDestination({...empty,node:node('project')})).toEqual({to:'/project/$projectId',params:{projectId:'returned'}});
    expect(acceptedDestination({...empty,node:node('topic')})).toEqual({to:'/topic/$topicId',params:{topicId:'returned'}});
    expect(acceptedDestination({...empty,node:node('thread')})).toEqual({to:'/thread/$threadId',params:{threadId:'returned'}});
  });
  it('opens the permitted chat without reading or guessing its private org',() => {
    expect(acceptedDestination({...empty,chat:{id:'chat',kind:'group',org_id:'returned-root-id',node:null,name:'Crew'}})).toEqual({to:'/org/$slug/messages',params:{slug:'returned-root-id'},search:{chat:'chat'}});
    expect(acceptedDestination({...empty,node:{...node('topic'),name:null,redacted:true},chat:{id:'chat-only',kind:'node',org_id:'root',node:{...node('topic'),name:null,redacted:true},name:null}})).toEqual({to:'/org/$slug/messages',params:{slug:'root'},search:{chat:'chat-only'}});
  });
  it('keeps a joined org’s returned address and never derives a missing target from a preview',() => {
    const joined=org('org','research','Research',userRef(me(1,'ada','Ada')),{address:'acme.research'});
    expect(acceptedDestination({...empty,org:joined})).toEqual({to:'/org/$slug',params:{slug:'acme.research'}});
    expect(acceptedDestination({...empty,node:{...node('topic'),redacted:true}})).toEqual({to:'/'});
    expect(acceptedDestination(empty)).toEqual({to:'/'});
  });
});
