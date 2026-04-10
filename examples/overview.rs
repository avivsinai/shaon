use anyhow::Result;
use chrono::{Datelike, Local};
use hilan::core::AttendanceProvider;

#[tokio::main]
async fn main() -> Result<()> {
    let config = hilan::Config::load()?;
    let mut provider = hilan::build_authenticated_provider(config).await?;
    let today = Local::now().date_naive();
    let month = today.with_day(1).expect("valid first day of month");
    let overview = hilan::use_cases::build_overview(&mut provider, month, today).await?;

    println!(
        "{}: {}/{} reported, {} missing, {} errors",
        overview.month.format("%Y-%m"),
        overview.summary.reported,
        overview.summary.total_work_days,
        overview.summary.missing,
        overview.summary.errors
    );

    for suggestion in overview.suggested_actions {
        println!("suggested action: {suggestion:?}");
    }

    let _ = AttendanceProvider::attendance_types(&mut provider).await?;

    Ok(())
}
