use shaon::core::{
    AbsenceProvider, AttendanceProvider, PayslipProvider, ReportProvider, SalaryProvider,
};
use shaon::provider::HilanProvider;

fn assert_core_traits<T>()
where
    T: AttendanceProvider + SalaryProvider + PayslipProvider + ReportProvider + AbsenceProvider,
{
}

#[test]
fn shaon_provider_implements_the_core_trait_stack() {
    assert_core_traits::<HilanProvider>();
}
