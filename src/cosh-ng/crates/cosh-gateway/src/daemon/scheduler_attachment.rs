//! Production scheduler attachment for sealed Gateway capability profiles.

use super::*;

impl GatewayDaemon {
    /// Enables scheduling with an explicitly injected generic brokered driver.
    ///
    /// The production entrypoint reaches this boundary only after its concrete
    /// provider has passed profile, target, socket, workspace, peer, and audit
    /// admission. Supplying a driver here is the explicit authority wiring;
    /// merely selecting a capability profile does not construct a target.
    ///
    /// # Errors
    ///
    /// Returns when a scheduler is already attached or durable state cannot be
    /// opened under the same installation identity.
    pub fn attach_brokered_scheduler(
        &mut self,
        containment: VerifiedRuntimeContainment,
        worker_id: BoundedOpaque,
        factory: Box<dyn RuntimeFactory>,
        driver: Box<dyn BrokeredExecutionDriver>,
    ) -> Result<(), GatewayDaemonError> {
        if self.scheduler.is_some() {
            return Err(GatewayDaemonError::Protocol(
                "Gateway scheduler is already attached".to_owned(),
            ));
        }
        self.scheduler = Some(
            TaskScheduler::open_for_capability_profile(
                &self.database_path,
                Some(self.coordinator.installation_id.clone()),
                worker_id,
                self.capability_profile,
                factory,
            )?
            .with_brokered_execution_driver(driver),
        );
        self.runtime_containment = Some(containment);
        Ok(())
    }

    /// Enables task-only scheduling with the default rejecting brokered driver.
    ///
    /// Generic Capability, Approval, Permit, and Execution contracts remain
    /// available to the scheduler, but this production attachment does not
    /// install a target provider. Brokered Runtime requests therefore fail
    /// closed before any external side effect can be attempted.
    ///
    /// # Errors
    ///
    /// Returns when a scheduler is already attached or durable state cannot be
    /// opened under the same installation identity.
    pub fn attach_task_only_scheduler(
        &mut self,
        containment: VerifiedRuntimeContainment,
        worker_id: BoundedOpaque,
        factory: Box<dyn RuntimeFactory>,
    ) -> Result<(), GatewayDaemonError> {
        if self.capability_profile != GatewayCapabilityProfile::task_only_v1() {
            return Err(GatewayDaemonError::Protocol(
                "provider capability profiles require an explicit brokered driver".to_owned(),
            ));
        }
        if self.scheduler.is_some() {
            return Err(GatewayDaemonError::Protocol(
                "Gateway scheduler is already attached".to_owned(),
            ));
        }
        self.scheduler = Some(TaskScheduler::open_for_capability_profile(
            &self.database_path,
            Some(self.coordinator.installation_id.clone()),
            worker_id,
            self.capability_profile,
            factory,
        )?);
        self.runtime_containment = Some(containment);
        Ok(())
    }
}
