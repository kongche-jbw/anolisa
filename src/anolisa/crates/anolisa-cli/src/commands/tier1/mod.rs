//! Tier 1 — Capability commands.
//!
//! These are capability-vocabulary verbs for everyday use. Each verb operates on
//! a capability noun (e.g. `token-optimization`); component / feature / target
//! names must not leak out to the user-facing surface (see Layer Discipline in
//! the design doc).

pub mod list;
pub mod enable;
pub mod disable;
pub mod status;
pub mod doctor;
pub mod logs;
pub mod restart;
pub mod env;
pub mod info;
pub mod update;
