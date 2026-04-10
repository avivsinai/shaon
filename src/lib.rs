pub use hr_core as core;
pub use hr_core::use_cases;
pub use provider_hilan::{
    api, attendance, build_authenticated_provider, build_provider, client, config, ontology,
    provider, reports, Config, HilanProvider,
};
pub use shaon_mcp as mcp;

pub mod app {
    pub use shaon_cli as cli;
    pub use shaon_mcp as mcp;
}
