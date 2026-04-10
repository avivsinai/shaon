use hilan::core::{
    AbsenceProvider, AttendanceProvider, PayslipProvider, ReportProvider, SalaryProvider,
};
use hilan::provider::HilanProvider;

fn assert_core_traits<T>()
where
    T: AttendanceProvider + SalaryProvider + PayslipProvider + ReportProvider + AbsenceProvider,
{
}

#[test]
fn hilan_provider_implements_the_core_trait_stack() {
    assert_core_traits::<HilanProvider>();
}
