// Helpers for upcoming attendance features — not yet wired to CLI commands.
#![allow(dead_code)]

use anyhow::{Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

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

#[derive(Debug, Deserialize, Serialize)]
pub struct SalaryTableColumn {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "DisplayName")]
    pub display_name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SalaryInitialData {
    #[serde(rename = "SelectedDates", default)]
    pub selected_dates: Vec<String>,
    #[serde(rename = "SelectedSingleDate")]
    pub selected_single_date: Option<String>,
    #[serde(rename = "TableColumns", default)]
    pub table_columns: Vec<SalaryTableColumn>,
    #[serde(rename = "TableData", default)]
    pub table_data: Vec<BTreeMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Serialize, Eq, PartialEq, Ord, PartialOrd)]
pub struct ErrorTask {
    pub date: NaiveDate,
    pub report_id: String,
    pub error_type: String,
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

pub(crate) fn parse_salary_initial_data(text: &str) -> serde_json::Result<SalaryInitialData> {
    serde_json::from_str(text)
}

pub(crate) fn parse_error_tasks(text: &str) -> serde_json::Result<Vec<ErrorTask>> {
    let value: serde_json::Value = serde_json::from_str(text)?;
    Ok(extract_error_tasks(&value))
}

fn extract_error_tasks(value: &serde_json::Value) -> Vec<ErrorTask> {
    let mut seen = BTreeSet::new();
    let mut tasks = Vec::new();
    collect_error_tasks(value, &mut seen, &mut tasks);
    tasks
}

fn collect_error_tasks(
    value: &serde_json::Value,
    seen: &mut BTreeSet<ErrorTask>,
    tasks: &mut Vec<ErrorTask>,
) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                collect_error_tasks(item, seen, tasks);
            }
        }
        serde_json::Value::Object(map) => {
            for key in ["link", "Link"] {
                if let Some(link) = map.get(key).and_then(serde_json::Value::as_str) {
                    if let Some(task) = parse_error_task_link(link) {
                        if seen.insert(task.clone()) {
                            tasks.push(task);
                        }
                    }
                }
            }

            for child in map.values() {
                collect_error_tasks(child, seen, tasks);
            }
        }
        _ => {}
    }
}

fn parse_error_task_link(link: &str) -> Option<ErrorTask> {
    if !link.contains("EmployeeErrorHandling.aspx") {
        return None;
    }

    let (_, query) = link.split_once('?')?;
    let mut date = None;
    let mut report_id = None;
    let mut error_type = None;

    for pair in query.split('&') {
        let (key, value) = pair.split_once('=')?;
        let value = urlencoding::decode(value).ok()?.into_owned();
        match key {
            "date" => date = NaiveDate::parse_from_str(&value, "%d/%m/%Y").ok(),
            "reportId" => report_id = Some(value),
            "errorType" => error_type = Some(value),
            _ => {}
        }
    }

    Some(ErrorTask {
        date: date?,
        report_id: report_id?,
        error_type: error_type?,
    })
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

/// Fetch salary summary initial data from the payments-and-deductions API.
///
/// Calls `PaymentsAndDeductionsApiapi.asmx/GetInitialData`.
pub async fn get_salary_initial(client: &mut HilanClient) -> Result<SalaryInitialData> {
    let text: String = client
        .asmx_call("PaymentsAndDeductionsApiapi", "GetInitialData")
        .await
        .context("get_salary_initial")?;

    parse_salary_initial_data(&text)
        .context("parse JSON from PaymentsAndDeductionsApiapi/GetInitialData")
}

/// Fetch error tasks from the home tasks API and extract fix parameters.
///
/// Calls `HHomeTasksApiapi.asmx/GetData`.
pub async fn get_error_tasks(client: &mut HilanClient) -> Result<Vec<ErrorTask>> {
    let text: String = client
        .asmx_call("HHomeTasksApiapi", "GetData")
        .await
        .context("get_error_tasks")?;

    parse_error_tasks(&text).context("parse JSON from HHomeTasksApiapi/GetData")
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

        assert_eq!(data.principal_user.user_id, "999999");
        assert_eq!(data.principal_user.employee_id, 99);
        assert_eq!(data.organization_id.as_deref(), Some("1234"));
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

    #[test]
    fn live_salary_get_initial_data_fixture_exposes_table_data() {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/asmx/salary-GetInitialData-full.json"
        ));

        let data = parse_salary_initial_data(text).expect("parse salary GetInitialData fixture");

        assert_eq!(data.selected_single_date.as_deref(), Some("03/2026"));
        assert_eq!(data.selected_dates, vec!["03/2026"]);
        assert_eq!(data.table_data.len(), 1);
        assert_eq!(
            data.table_columns
                .iter()
                .find(|column| column.name == "Bruto")
                .map(|column| column.display_name.as_str()),
            Some("ברוטו")
        );
        assert_eq!(
            data.table_data[0]
                .get("Range")
                .and_then(serde_json::Value::as_str),
            Some("מרץ 2026")
        );
        assert_eq!(
            data.table_data[0]
                .get("Bruto")
                .and_then(serde_json::Value::as_f64),
            Some(12345.0)
        );
    }

    #[test]
    fn tasks_get_data_fixture_extracts_error_fix_params() {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/asmx/tasks-GetData.json"
        ));

        let tasks = parse_error_tasks(text).expect("parse tasks GetData fixture");

        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].date, NaiveDate::from_ymd_opt(2026, 4, 9).unwrap());
        assert_eq!(tasks[0].report_id, "00000000-0000-0000-0000-000000000000");
        assert_eq!(tasks[0].error_type, "63");
        assert_eq!(tasks[1].date, NaiveDate::from_ymd_opt(2026, 4, 6).unwrap());
        assert_eq!(tasks[1].report_id, "00000000-0000-0000-0000-000000000000");
        assert_eq!(tasks[1].error_type, "63");
    }
}
