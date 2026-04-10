// Helpers for upcoming attendance features — not yet wired to CLI commands.
#![allow(dead_code)]

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::client::HilanClient;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Serialize)]
pub struct BootstrapInfo {
    pub user_id: String,
    pub employee_id: u32,
    pub org_id: String,
    pub name: String,
    pub is_manager: bool,
}

#[derive(Debug, Deserialize)]
pub struct TasksCount {
    #[serde(rename = "TasksCount")]
    pub tasks_count: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AbsenceSymbol {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "DisplayName")]
    pub display_name: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AbsencesInitialData {
    #[serde(rename = "Symbols")]
    pub symbols: Vec<AbsenceSymbol>,
}

#[derive(Deserialize)]
struct GetDataResponse {
    #[serde(rename = "PrincipalUser")]
    principal_user: PrincipalUserRaw,
    #[serde(rename = "OrganizationId")]
    organization_id: Option<String>,
}

#[derive(Deserialize)]
struct PrincipalUserRaw {
    #[serde(rename = "UserId")]
    user_id: String,
    #[serde(rename = "EmployeeId")]
    employee_id: u32,
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "IsManager")]
    is_manager: bool,
    #[serde(rename = "OrganizationId")]
    organization_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum TasksCountResponse {
    RawNumber(u32),
    WrappedObject {
        #[serde(rename = "TasksCount")]
        tasks_count: u32,
    },
}

fn parse_bootstrap_info(text: &str) -> serde_json::Result<BootstrapInfo> {
    let data: GetDataResponse = serde_json::from_str(text)?;
    let user = data.principal_user;
    let org_id = data
        .organization_id
        .or(user.organization_id)
        .ok_or_else(|| {
            serde_json::Error::io(std::io::Error::other(
                "bootstrap: OrganizationId not found in GetData response",
            ))
        })?;

    Ok(BootstrapInfo {
        user_id: user.user_id,
        employee_id: user.employee_id,
        org_id,
        name: user.name,
        is_manager: user.is_manager,
    })
}

fn parse_tasks_count(text: &str) -> serde_json::Result<TasksCount> {
    match serde_json::from_str::<TasksCountResponse>(text)? {
        TasksCountResponse::RawNumber(tasks_count) => Ok(TasksCount { tasks_count }),
        TasksCountResponse::WrappedObject { tasks_count } => Ok(TasksCount { tasks_count }),
    }
}

fn parse_absences_initial_data(text: &str) -> serde_json::Result<AbsencesInitialData> {
    serde_json::from_str(text)
}

// ---------------------------------------------------------------------------
// Public API functions
// ---------------------------------------------------------------------------

/// Fetch bootstrap employee info from the employee strip API.
///
/// Calls `HEmployeeStripApiapi.asmx/GetData` and extracts identity fields
/// from the `PrincipalUser` object in the response.
pub async fn bootstrap(client: &mut HilanClient) -> Result<BootstrapInfo> {
    let text: String = client
        .asmx_call("HEmployeeStripApiapi", "GetData")
        .await
        .context("bootstrap: GetData")?;

    parse_bootstrap_info(&text).context("parse JSON from HEmployeeStripApiapi/GetData")
}

/// Fetch the pending-tasks count from the home page API.
///
/// Calls `HHomeTasksApiapi.asmx/GetTasksCount`.
pub async fn get_tasks_count(client: &mut HilanClient) -> Result<TasksCount> {
    let text: String = client
        .asmx_call("HHomeTasksApiapi", "GetTasksCount")
        .await
        .context("get_tasks_count")?;

    parse_tasks_count(&text).context("parse JSON from HHomeTasksApiapi/GetTasksCount")
}

/// Fetch absences initial data (symbols / attendance-type list).
///
/// Calls `HAbsencesApiapi.asmx/GetInitialData`.
pub async fn get_absences_initial(client: &mut HilanClient) -> Result<AbsencesInitialData> {
    let text: String = client
        .asmx_call("HAbsencesApiapi", "GetInitialData")
        .await
        .context("get_absences_initial")?;

    parse_absences_initial_data(&text).context("parse JSON from HAbsencesApiapi/GetInitialData")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_get_data_fixture_parses_without_d_wrapper() {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/asmx/GetData.json"
        ));

        let data: GetDataResponse = serde_json::from_str(text).expect("parse GetData fixture");

        assert_eq!(data.principal_user.user_id, "460627");
        assert_eq!(data.principal_user.employee_id, 27);
        assert_eq!(data.organization_id.as_deref(), Some("4606"));
    }

    #[test]
    fn live_get_initial_data_fixture_parses_without_d_wrapper() {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/asmx/GetInitialData.json"
        ));

        let data = parse_absences_initial_data(text).expect("parse GetInitialData fixture");

        assert_eq!(data.symbols.len(), 2);
        assert_eq!(data.symbols[0].id, "481");
        assert_eq!(data.symbols[0].name, "חופשה");
    }

    #[test]
    fn live_get_tasks_count_fixture_parses_plain_number_response() {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/asmx/GetTasksCount.json"
        ));

        let data = parse_tasks_count(text).expect("parse GetTasksCount fixture");

        assert_eq!(data.tasks_count, 0);
    }
}
