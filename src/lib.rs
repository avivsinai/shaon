pub use hilan_mcp as mcp;
pub use hr_core as core;
pub use hr_core::use_cases;
pub use provider_hilan::{
    api, attendance, build_authenticated_provider, build_provider, client, config, ontology,
    provider, reports, Config, HilanProvider,
};

pub mod app {
    pub use hilan_cli as cli;
    pub use hilan_mcp as mcp;
}
