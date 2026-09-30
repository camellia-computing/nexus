pub mod config_service;
pub mod configuration;
pub mod configuration_assessment;
mod configuration_field_evidence;
pub mod controller;
mod core_build_constraints;
pub mod core_compatibility;
pub mod core_knowledge;
pub mod core_probe;
pub mod error;
pub mod jsonc;
pub mod manager;
pub mod model;
mod native_diagnostics;
pub mod plans;
pub mod ports;
pub mod privileges;
pub mod programs;
pub mod share_compatibility;

pub use config_service::{ConfigService, PreparedConfigGuard};
pub use configuration::*;
pub use configuration_assessment::*;
pub use controller::{ControllerHandle, Mutation};
pub use core_compatibility::*;
pub use core_knowledge::*;
pub use core_probe::*;
pub use error::{CamelliaNexusError, ErrorCode, Result};
pub use jsonc::normalize_jsonc;
pub use manager::{
    AutoStartReport, CreateProgramRequest, PreparedPackageGuard, PreparedProgramCreate,
    PreparedProgramUpdate, ProgramManager, StopActiveReport,
};
pub use model::*;
pub use native_diagnostics::NativeDiagnosticReport;
pub use plans::*;
pub use ports::*;
pub use privileges::*;
pub use programs::{AdapterRegistry, ProgramAdapter};
pub use share_compatibility::*;
