// Helpers for upcoming attendance features — not yet wired to CLI commands.
#![allow(dead_code)]

use anyhow::{anyhow, Context, Result};
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

// ---------------------------------------------------------------------------
// Internal response wrappers (match Hilan JSON shapes)
// ---------------------------------------------------------------------------

/// The `d` wrapper that Hilan ASMX endpoints return.
#[derive(Deserialize)]
struct AsmxWrapper<T> {
    d: T,
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

// ---------------------------------------------------------------------------
// Public API functions
// ---------------------------------------------------------------------------

/// Fetch bootstrap employee info from the employee strip API.
///
/// Calls `HEmployeeStripApiapi.asmx/GetData` and extracts identity fields
/// from the `PrincipalUser` object in the response.
pub async fn bootstrap(client: &HilanClient) -> Result<BootstrapInfo> {
    // Hilan ASMX endpoints wrap their payload in { "d": ... }
    let wrapper: AsmxWrapper<GetDataResponse> = client
        .asmx_call("HEmployeeStripApiapi", "GetData")
        .await
        .context("bootstrap: GetData")?;

    let data = wrapper.d;
    let user = data.principal_user;

    // OrganizationId can appear at root level or inside PrincipalUser
    let org_id = data
        .organization_id
        .or(user.organization_id)
        .ok_or_else(|| anyhow!("bootstrap: OrganizationId not found in GetData response"))?;

    Ok(BootstrapInfo {
        user_id: user.user_id,
        employee_id: user.employee_id,
        org_id,
        name: user.name,
        is_manager: user.is_manager,
    })
}

/// Fetch the pending-tasks count from the home page API.
///
/// Calls `HHomeTasksApiapi.asmx/GetTasksCount`.
pub async fn get_tasks_count(client: &HilanClient) -> Result<TasksCount> {
    let wrapper: AsmxWrapper<TasksCount> = client
        .asmx_call("HHomeTasksApiapi", "GetTasksCount")
        .await
        .context("get_tasks_count")?;

    Ok(wrapper.d)
}

/// Fetch absences initial data (symbols / attendance-type list).
///
/// Calls `HAbsencesApiapi.asmx/GetInitialData`.
pub async fn get_absences_initial(client: &HilanClient) -> Result<AbsencesInitialData> {
    let wrapper: AsmxWrapper<AbsencesInitialData> = client
        .asmx_call("HAbsencesApiapi", "GetInitialData")
        .await
        .context("get_absences_initial")?;

    Ok(wrapper.d)
}
