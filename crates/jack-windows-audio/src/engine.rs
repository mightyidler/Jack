//! Real transport adapter for the journaled runtime controller. Never routes
//! applications itself; source identity is rechecked before prepare and start.
use crate::{
    sources,
    transport::{self, PreparedTransport, RunningTransport},
};
use jack_core::runtime::{AudioEngine, EnginePlan};
use uuid::Uuid;

pub struct PreparedEngine {
    transport: PreparedTransport,
    plan: EnginePlan,
}
#[derive(Default)]
pub struct NativeEngine {
    running: Option<RunningTransport>,
}
impl AudioEngine for NativeEngine {
    type Prepared = PreparedEngine;
    fn prepare(&mut self, plan: &EnginePlan) -> Result<Self::Prepared, String> {
        if self.running.is_some() {
            return Err("audio transport already exists".into());
        }
        sources::verify_plan(plan)?;
        Ok(PreparedEngine {
            transport: transport::prepare(plan)?,
            plan: plan.clone(),
        })
    }
    fn start(&mut self, prepared: Self::Prepared) -> Result<(), String> {
        if self.running.is_some() {
            return Err("audio transport already exists".into());
        }
        sources::verify_plan(&prepared.plan)?;
        self.running = Some(prepared.transport.start()?);
        Ok(())
    }
    fn stop(&mut self) -> Result<(), String> {
        if let Some(mut running) = self.running.take() {
            let report = running.stop();
            if !report.errors.is_empty() {
                return Err(format!("transport stop: {:?}", report.errors));
            }
        }
        Ok(())
    }
    fn healthy(&self) -> bool {
        self.running.as_ref().is_some_and(RunningTransport::healthy)
    }
    fn set_group_gain(&mut self, group: Uuid, gain: f32) -> Result<(), String> {
        self.running
            .as_ref()
            .ok_or("audio transport is stopped")?
            .set_group_gain(group, gain)
    }
}
