pub mod config_service;
pub mod configuration;
pub mod controller;
pub mod core_compatibility;
pub mod core_versions;
pub mod error;
pub mod jsonc;
pub mod manager;
pub mod model;
pub mod plans;
pub mod ports;
pub mod privileges;
pub mod programs;
pub mod share_compatibility;

pub use config_service::{ConfigService, PreparedConfigGuard};
pub use configuration::*;
pub use controller::{ControllerHandle, Mutation};
pub use core_compatibility::*;
pub use core_versions::*;
pub use error::{CamelliaNexusError, ErrorCode, Result};
pub use jsonc::normalize_jsonc;
pub use manager::{
    AutoStartReport, CreateProgramRequest, PreparedPackageGuard, PreparedProgramCreate,
    PreparedProgramUpdate, ProgramManager, StopActiveReport,
};
pub use model::*;
pub use plans::*;
pub use ports::*;
pub use privileges::*;
pub use programs::{AdapterRegistry, ProgramAdapter};
pub use share_compatibility::*;
