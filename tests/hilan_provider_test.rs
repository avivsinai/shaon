use hilan::config::Config;
use hilan::core::{AttendanceProvider, PayslipProvider, ReportProvider, SalaryProvider};
use hilan::provider::HilanProvider;

fn test_config() -> Config {
    Config {
        subdomain: "acme".to_string(),
        username: "12345".to_string(),
        password: Some("s3cret".to_string()),
        payslip_folder: None,
        payslip_format: None,
    }
}

fn assert_core_traits<T>()
where
    T: AttendanceProvider + SalaryProvider + PayslipProvider + ReportProvider,
{
}

#[test]
fn hilan_provider_implements_the_core_trait_stack() {
    assert_core_traits::<HilanProvider>();
}

#[test]
fn hilan_provider_reports_current_capabilities() {
    let provider = HilanProvider::new(test_config()).expect("build provider");
    let caps = provider.capabilities();

    assert!(caps.attendance_read);
    assert!(caps.attendance_write);
    assert!(caps.fix_errors);
    assert!(caps.attendance_types);
    assert!(caps.salary_summary);
    assert!(caps.payslips);
    assert!(caps.reports);
}
