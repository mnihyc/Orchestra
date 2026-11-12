//! Scope the existing common approval producer, without changing its ordinary read or decision doors.

pub use super::*;

pub use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::ports::PortOpening;

use orch_api_contract::ports::PortTarget;

use std::collections::BTreeSet;
orch_api_base::api_family_callback! {
    bind __api_bind_d708c28645, __api_install_d708c28645, __api_callback_d708c28645;
    slot __ApiCallbackd708c28645, __API_CALLBACK_D708C28645, __api_set_d708c28645;
    base orch_api_base::routes::approvals::operation;

    async fn governor<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_db::queries::approvals::ApprovalRow) -> orch_api_base::middleware::requests::ApiResult<orch_access_contract::key_scope::KeyResolvedTarget>;
}

#[derive(Debug, Clone, Default)]
pub struct Sources {
    pub objects: BTreeSet<(ObjectKind, Id)>,
    pub chats: BTreeSet<Id>,
}

impl Sources {
    /// Actual named objects selected by the existing native card source walk.
    pub fn objects(&self) -> impl Iterator<Item = (ObjectKind, Id)> + '_ {
        self.objects.iter().copied()
    }
    /// Actual named chats selected by that same walk.
    pub fn chats(&self) -> impl Iterator<Item = Id> + '_ {
        self.chats.iter().copied()
    }
    pub fn object(&mut self, kind: ObjectKind, id: Id) {
        if !id.is_nil() {
            self.objects.insert((kind, id));
        }
    }
    pub fn node(&mut self, node: &orch_api_contract::nodes::NodeRef) {
        if node.name.is_some() || node.thread_display.is_some() {
            let kind = match node.kind {
                orch_api_contract::nodes::NodeKind::Org => ObjectKind::Org,
                orch_api_contract::nodes::NodeKind::Project => ObjectKind::Project,
                orch_api_contract::nodes::NodeKind::Topic => ObjectKind::Topic,
                orch_api_contract::nodes::NodeKind::Thread => ObjectKind::Thread,
            };
            self.object(kind, node.id);
        }
    }
    pub fn opening(&mut self, opening: &PortOpening) {
        self.object(ObjectKind::Environment, opening.environment_id);
        if let Some(node) = &opening.place {
            self.node(node);
        }
        if let Some(node) = &opening.requested_by_thread {
            self.node(node);
        }
        if let Some(scope) = opening
            .admission
            .as_ref()
            .and_then(|admission| admission.scope.as_ref())
        {
            self.node(scope);
        }
        if let Some(host) = &opening.host {
            self.object(ObjectKind::Host, host.id);
        }
        if let Some(machine) = &opening.publisher {
            self.object(ObjectKind::Machine, machine.id);
        }
    }
    pub fn port_target(&mut self, target: &PortTarget) {
        self.object(ObjectKind::Environment, target.environment_id);
        for opening in &target.openings {
            self.opening(opening);
        }
    }
    pub fn approval(&mut self, approval: &Approval) -> ApiResult<()> {
        if let Some(node) = &approval.node {
            self.node(node);
        }
        if let ApprovalRequester::Thread(thread) = &approval.requester
            && !thread.name.is_empty()
        {
            self.object(ObjectKind::Thread, thread.id);
        }
        if let Some(object) = &approval.object
            && !object.name.is_empty()
        {
            if object.kind == "chat" {
                self.chats.insert(object.id);
            } else {
                let kind = crate::routes::access::subject::kind_of_word(&object.kind).ok_or_else(
                    || ApiError::internal("the approval has an untyped named object"),
                )?;
                self.object(kind, object.id);
            }
        }
        match &approval.request {
            ApprovalRequest::Capability(request) => {
                self.object(ObjectKind::Environment, request.environment.id);
                self.object(ObjectKind::Host, request.host.id);
            }
            ApprovalRequest::Access(request) => {
                if let Some(node) = &request.for_thread {
                    self.node(node);
                }
                match &request.holder {
                    orch_api_contract::access::Holder::User { .. } => {}
                    orch_api_contract::access::Holder::Node { node } => self.node(node),
                    // The native access-request producer accepts selected people or work only.
                    // A later producer extension must declare its actual Group/role sources;
                    // the broad shared Holder type is not an implicit disclosure grant.
                    orch_api_contract::access::Holder::Members { .. }
                    | orch_api_contract::access::Holder::Role { .. }
                    | orch_api_contract::access::Holder::Group { .. }
                    | orch_api_contract::access::Holder::Agents { .. }
                    | orch_api_contract::access::Holder::Everyone => {
                        return Err(ApiError::internal(
                            "the native approval has an undeclared holder source",
                        ));
                    }
                }
            }
            ApprovalRequest::Port(request) => {
                self.object(ObjectKind::Environment, request.environment.id);
                self.opening(&request.opening);
                self.port_target(&request.plan.target);
                if let Some(node) = &request.plan.audience.place {
                    self.node(node);
                }
                if let Some(candidate) = &request.plan.candidate {
                    if let Some(host) = &candidate.host {
                        self.object(ObjectKind::Host, host.id);
                    }
                    self.object(ObjectKind::Machine, candidate.machine.id);
                }
            }
            ApprovalRequest::Harness(_) | ApprovalRequest::Plan(_) | ApprovalRequest::Input(_) => {}
        }
        Ok(())
    }
    pub fn reply(reply: &ApprovalDecisionReply) -> ApiResult<Self> {
        let mut sources = Self::default();
        sources.approval(&reply.approval)?;
        if let Some(result) = &reply.opening_result {
            sources.opening(&result.opening);
        }
        Ok(sources)
    }
}

orch_api_base::api_family_callback! {
    bind __api_bind_7b89426127, __api_install_7b89426127, __api_callback_7b89426127;
    slot __ApiCallback7b89426127, __API_CALLBACK_7B89426127, __api_set_7b89426127;
    base orch_api_base::routes::approvals::operation;

    async fn bind_card_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api mut orch_api_base::key_operations::admission::Prepared, arg2: &'__api [orch_api_contract::approvals::Approval]) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_083f6d23de, __api_install_083f6d23de, __api_callback_083f6d23de;
    slot __ApiCallback083f6d23de, __API_CALLBACK_083F6D23DE, __api_set_083f6d23de;
    base orch_api_base::routes::approvals::operation;

    fn card_sources<'__api>(arg0: &'__api [orch_api_contract::approvals::Approval]) -> orch_api_base::middleware::requests::ApiResult<orch_api_base::routes::approvals::operation::Sources>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_95d9a090f1, __api_install_95d9a090f1, __api_callback_95d9a090f1;
    slot __ApiCallback95d9a090f1, __API_CALLBACK_95D9A090F1, __api_set_95d9a090f1;
    base orch_api_base::routes::approvals::operation;

    async fn bind_prepared_card_sources<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api mut orch_api_base::key_operations::admission::Prepared, arg2: &'__api orch_api_base::routes::approvals::operation::Sources) -> orch_api_base::middleware::requests::ApiResult<()>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_b636a30341, __api_install_b636a30341, __api_callback_b636a30341;
    slot __ApiCallbackb636a30341, __API_CALLBACK_B636A30341, __api_set_b636a30341;
    base orch_api_base::routes::approvals::operation;

    fn card_read_caller<'__api>(arg0: &'__api orch_authz::Caller, arg1: &'__api orch_api_base::key_operations::admission::Prepared, arg2: &'static str, arg3: &'__api [orch_access_contract::key_scope::KeyResolvedTarget]) -> orch_api_base::middleware::requests::ApiResult<Option<orch_authz::Caller>>;
}
