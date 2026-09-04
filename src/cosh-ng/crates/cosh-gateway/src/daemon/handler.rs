//! Transport-neutral Task request admission and dispatch.

use cosh_gateway_contracts::common::{ActorRef, RuntimeSelector, TargetRef, WorkspaceRef};
use cosh_gateway_contracts::ids::{ActorId, TaskId};
use cosh_gateway_contracts::profile::GatewayCapabilityProfileId;

use super::{
    AppendTaskInput, CancelTask, GatewayAdmission, GatewayDaemonError, GatewayRequest,
    GatewayResult, ResolveApproval, RetryTask, SubmitTask, TaskEventPage, TaskView,
    GATEWAY_API_VERSION_V1, GATEWAY_API_VERSION_V2,
};

/// Mutating Task operations available to the transport handler.
pub(super) trait TaskCommandPort {
    fn submit(
        &mut self,
        actor: &ActorRef,
        workspace: &WorkspaceRef,
        request: SubmitTask,
    ) -> Result<TaskView, GatewayDaemonError>;

    fn cancel(
        &mut self,
        actor_id: &ActorId,
        request: CancelTask,
    ) -> Result<TaskView, GatewayDaemonError>;

    fn retry(
        &mut self,
        actor: &ActorRef,
        target: &TargetRef,
        workspace: &WorkspaceRef,
        runtime: &RuntimeSelector,
        request: RetryTask,
    ) -> Result<TaskView, GatewayDaemonError>;

    fn resolve_approval(
        &mut self,
        actor_id: &ActorId,
        request: ResolveApproval,
    ) -> Result<TaskView, GatewayDaemonError>;

    fn append_input(
        &mut self,
        actor_id: &ActorId,
        request: AppendTaskInput,
    ) -> Result<TaskView, GatewayDaemonError>;
}

/// Read-only Task projections available to the transport handler.
pub(super) trait TaskProjectionPort {
    fn get(&self, actor_id: &ActorId, task_id: &TaskId) -> Result<TaskView, GatewayDaemonError>;

    fn events(
        &self,
        actor_id: &ActorId,
        task_id: &TaskId,
        after_revision: Option<u64>,
        limit: u16,
    ) -> Result<TaskEventPage, GatewayDaemonError>;
}

/// Trusted admission values selected before request dispatch.
pub(super) struct TaskAdmission<'a> {
    pub(super) snapshot: &'a GatewayAdmission,
}

/// Dispatches one authenticated request through Task command and projection ports.
pub(super) fn dispatch<P>(
    actor: &ActorRef,
    request: GatewayRequest,
    admission: TaskAdmission<'_>,
    ports: &mut P,
) -> Result<GatewayResult, GatewayDaemonError>
where
    P: TaskCommandPort + TaskProjectionPort,
{
    validate_request_version_shape(&request)?;
    match request {
        GatewayRequest::Ping { .. } => Ok(GatewayResult::Pong),
        GatewayRequest::Admission { .. } => {
            Ok(GatewayResult::Admission(admission.snapshot.clone()))
        }
        GatewayRequest::Submit {
            api_version,
            admission: claimed,
            request,
        } => {
            match (api_version.as_str(), claimed.as_deref()) {
                (GATEWAY_API_VERSION_V1, None) => {
                    validate_legacy_submission(&request, admission.snapshot)?;
                }
                (GATEWAY_API_VERSION_V2, Some(claimed)) => {
                    validate_submission_admission(&request, claimed, admission.snapshot)?;
                }
                _ => unreachable!("version shape is validated before dispatch"),
            }
            ports
                .submit(actor, &admission.snapshot.workspace, request)
                .map(GatewayResult::Task)
        }
        GatewayRequest::Get { task_id, .. } => ports
            .get(&actor.actor_id, &task_id)
            .map(GatewayResult::Task),
        GatewayRequest::Events {
            task_id,
            after_revision,
            limit,
            ..
        } => ports
            .events(&actor.actor_id, &task_id, after_revision, limit)
            .map(GatewayResult::Events),
        GatewayRequest::Cancel { request, .. } => ports
            .cancel(&actor.actor_id, request)
            .map(GatewayResult::Cancelled),
        GatewayRequest::Retry { request, .. } => ports
            .retry(
                actor,
                &admission.snapshot.target,
                &admission.snapshot.workspace,
                &admission.snapshot.runtime,
                request,
            )
            .map(GatewayResult::Retried),
        GatewayRequest::ResolveApproval { request, .. } => ports
            .resolve_approval(&actor.actor_id, request)
            .map(GatewayResult::ApprovalResolved),
        GatewayRequest::AppendInput { request, .. } => ports
            .append_input(&actor.actor_id, request)
            .map(GatewayResult::InputAppended),
    }
}

pub(super) fn validate_request_version_shape(
    request: &GatewayRequest,
) -> Result<(), GatewayDaemonError> {
    if !matches!(
        request.api_version(),
        GATEWAY_API_VERSION_V1 | GATEWAY_API_VERSION_V2
    ) {
        return Err(GatewayDaemonError::Protocol(
            "unsupported Gateway API version".to_owned(),
        ));
    }
    match request {
        GatewayRequest::Admission { api_version, .. } if api_version != GATEWAY_API_VERSION_V2 => {
            Err(GatewayDaemonError::Protocol(
                "admission discovery requires cosh.gateway.v2".to_owned(),
            ))
        }
        GatewayRequest::Submit {
            api_version,
            admission: Some(_),
            ..
        } if api_version == GATEWAY_API_VERSION_V1 => Err(GatewayDaemonError::Protocol(
            "cosh.gateway.v1 submit must use the frozen request shape".to_owned(),
        )),
        GatewayRequest::Submit {
            api_version,
            admission: None,
            ..
        } if api_version == GATEWAY_API_VERSION_V2 => Err(GatewayDaemonError::Protocol(
            "cosh.gateway.v2 submit requires an admission echo".to_owned(),
        )),
        _ => Ok(()),
    }
}

pub(super) fn validate_legacy_submission(
    request: &SubmitTask,
    admitted: &GatewayAdmission,
) -> Result<(), GatewayDaemonError> {
    let profile = admitted.capability_profile.profile_id.profile();
    if admitted.capability_profile.profile_id != GatewayCapabilityProfileId::TaskOnlyV1
        || profile
            .verify_identity(&admitted.capability_profile)
            .is_err()
        || admitted.target != profile.governed_target()
        || request.target != admitted.target
        || request.runtime != admitted.runtime
    {
        return Err(GatewayDaemonError::Protocol(
            "cosh.gateway.v1 submit is admitted only by an exact task-only-v1 daemon".to_owned(),
        ));
    }
    Ok(())
}

pub(super) fn validate_submission_admission(
    request: &SubmitTask,
    claimed: &GatewayAdmission,
    admitted: &GatewayAdmission,
) -> Result<(), GatewayDaemonError> {
    let profile = admitted.capability_profile.profile_id.profile();
    if claimed != admitted
        || profile
            .verify_identity(&admitted.capability_profile)
            .is_err()
        || admitted.target != profile.governed_target()
        || request.target != admitted.target
        || request.runtime != admitted.runtime
    {
        return Err(GatewayDaemonError::Protocol(
            "Task admission, target, or Runtime is not admitted by this daemon".to_owned(),
        ));
    }
    Ok(())
}
