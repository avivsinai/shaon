use anyhow::Result;
use jiff::Zoned;
use shaon::core::AttendanceProvider;

#[tokio::main]
async fn main() -> Result<()> {
    let config = shaon::Config::load()?;
    let mut provider = shaon::build_authenticated_provider(config).await?;
    let today = Zoned::now().date();
    let month = today.first_of_month();
    let overview = shaon::use_cases::build_overview(&mut provider, month, today).await?;

    println!(
        "{}: {}/{} reported, {} missing, {} errors",
        overview.month.strftime("%Y-%m"),
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
