# Hilanet Protocol Map

Low-level reverse-engineering notes for shaon's Hilanet integration. This is the wire-format document, not the user guide.

Read these first if you want the higher-level view:

- [README.md](README.md) for install and usage
- [ARCHITECTURE.md](ARCHITECTURE.md) for crate boundaries and runtime flows
- [CLAUDE.md](CLAUDE.md) for maintainer instructions

Examples below come from an example Hilanet instance at `https://{subdomain}.hilan.co.il`, but the same patterns are intended to generalize across subdomains.

## Auth

### Fetch OrgId
```
GET https://{subdomain}.hilan.co.il/
```
Parse `"OrgId":"(\d+)"` from HTML/JS.

This is only needed before authentication, to submit the initial login request. Once authenticated, shaon prefers `HEmployeeStripApiapi.asmx/GetData` as the authoritative source of `OrganizationId`, `UserId`, and `EmployeeId`.

### Login
```
POST https://{subdomain}.hilan.co.il/HilanCenter/Public/api/LoginApi/LoginRequest
Content-Type: application/x-www-form-urlencoded

username={username}&password={password}&orgId={orgId}
```
Response JSON: `{ IsFail, IsShowCaptcha, Code, ErrorMessage }`
- `IsShowCaptcha: true` → CAPTCHA required (solve in browser)
- `Code: 18` → temp lockout
- `Code: 6` → password change required
- Sets session cookies for all subsequent requests

## Two Protocol Layers

Hilanet has two protocol layers:

### 1. Legacy ASP.NET WebForms (`.aspx` pages)
- Used by: attendance calendar, error wizard, analyzed sheet, meals, correction log
- Auth: session cookies
- Pattern: GET page → scrape and replay ALL hidden inputs from the fresh GET → override only edited fields + button name → POST back
- Form IDs embed employee ID + month: `ctl00$mp$RG_Days_{employeeId}_{YYYY}_{MM}`

### 2. Modern ASMX JSON API (`/Services/Public/WS/*.asmx/*`)
- Used by: absences, payslip, home page tasks, employee strip
- Auth: session cookies + sometimes H-XSRF-Token header
- Pattern: POST with `Content-Type: application/json` and `{}` body
- Response: JSON

## Key Identifiers

- **Subdomain**: `{subdomain}` → base URL `https://{subdomain}.hilan.co.il`
- **Employee ID (currentItemId)**: Extracted from `HEmployeeStripApiapi.asmx/GetData` → `PrincipalUser.UserId`
- **EmployeeId (short)**: `PrincipalUser.EmployeeId` (integer, e.g. 27)
- **OrganizationId**: `PrincipalUser.OrganizationId` (e.g. "4606")
- **UserId for payslips**: `{orgId}{username}` concatenated (no separator)
- **currentItemId** in calendar form = `PrincipalUser.UserId` from GetData API

### Bootstrap: Extract Employee Info
```
POST /Hilannetv2/Services/Public/WS/HEmployeeStripApiapi.asmx/GetData
Content-Type: application/json
Body: {}
```
Response contains: `PrincipalUser.UserId`, `PrincipalUser.EmployeeId`, `PrincipalUser.Name`, `OrganizationId`, `PrincipalUser.IsManager`

---

## Attendance Calendar (Read + Write)

### Read: Get Calendar Page
```
GET /Hilannetv2/Attendance/calendarpage.aspx?isOnSelf=true
```
Returns HTML with:
- Monthly calendar grid showing reported/error/missing days
- Bottom form for editing selected day's entry
- ASP.NET form with `__VIEWSTATE`, `__VIEWSTATEGENERATOR`

### Write: Submit Attendance Entry
```
POST /Hilannetv2/Attendance/calendarpage.aspx?isOnSelf=true
Content-Type: application/x-www-form-urlencoded
```

**Implementation strategy**: Full form replay. GET the page, parse ALL hidden inputs via HTML scraper, then POST back with all hidden fields unchanged + only the edited business fields overridden + the save button name. Do NOT hardcode a minimal field list — replay everything from the GET.

**No `__EVENTVALIDATION`** on this page. The H-XSRF-Token hidden field exists but is always empty (not enforced). Cookies are minimal: `h-culture=he-IL`, `HLoginLink={subdomain}`, plus session auth cookies from login.

Key form fields (full set scraped dynamically at runtime — do not hardcode):
| Field | Purpose | Example |
|-------|---------|---------|
| `__VIEWSTATE` | ASP.NET state (from GET) | `kCmX4ck...` |
| `__VIEWSTATEGENERATOR` | ASP.NET generator (from GET) | `152EA2C7` |
| `__EVENTTARGET` | Empty for button-triggered submit | `` |
| `__EVENTARGUMENT` | Empty | `` |
| `__calendarSelectedDays` | Day index in calendar | `9596` |
| `ctl00$mp$currentMonth` | Current month | `01/04/2026` |
| `ctl00$mp$Strip$hCurrentItemId` | Employee ID | `460627` |
| `ctl00$mp$RG_Days_{empId}_{YYYY}_{MM}$cellOf_ManualEntry_EmployeeReports_row_0_0$ManualEntry_EmployeeReports_row_0_0` | Entry time | `09:00` |
| `ctl00$mp$RG_Days_{empId}_{YYYY}_{MM}$cellOf_ManualExit_EmployeeReports_row_0_0$ManualExit_EmployeeReports_row_0_0` | Exit time | `18:00` |
| `ctl00$mp$RG_Days_{empId}_{YYYY}_{MM}$cellOf_Comment_EmployeeReports_row_0_0$Comment_EmployeeReports_row_0_0` | Comment | `` |
| `ctl00$mp$RG_Days_{empId}_{YYYY}_{MM}$cellOf_Symbol.SymbolId_EmployeeReports_row_0_0$Symbol.SymbolId_EmployeeReports_row_0_0` | Attendance type code | `120` (WFH) |
| `ctl00$mp$RG_Days_{empId}_{YYYY}_{MM}$cellOf_CompletionToStandard_EmployeeReports_row_0_0$CompletionToStandard_EmployeeReports_row_0_0` | Auto-fill to standard hours | `on` |
| `ctl00$mp$RG_Days_{empId}_{YYYY}_{MM}$btnSave` | Submit button | `שמירה` |

### Filter Buttons
- `ctl00$mp$RefreshSelectedDays` → "ימים נבחרים" (selected days)
- `ctl00$mp$RefreshErrorsDays` → "ימים שגויים" (error days)
- `ctl00$mp$RefreshPeriod` → "דיווח תקופתי" (period report)

---

## Error Wizard (אשף שגויים)

### Read: Error Day Form
```
GET /Hilannetv2/EmployeeErrorHandling.aspx?date={DD/MM/YYYY}&reportId=00000000-0000-0000-0000-000000000000&errorType=63&HideStrip=1&HideEmployeeStrip=1
```
(Note: canonical single-slash path; browsers normalize double-slash but we should use single)

Returns HTML with same form structure as calendar, pre-populated for the error day.

**Open question**: `reportId` and `errorType=63` may vary by error class. The sampled case was "חסר דיווח ליום עם תקן" (missing report for standard day). Other error types may use different `errorType` values. Verify before implementing multi-error-type support.

### Write: Fix Error Day
Same full-form-replay POST as attendance calendar but to the EmployeeErrorHandling URL:
```
POST /Hilannetv2/EmployeeErrorHandling.aspx?date={DD/MM/YYYY}&reportId=...&errorType=63&HideStrip=1&HideEmployeeStrip=1
```
Same form fields — `ManualEntry`, `ManualExit`, `Symbol.SymbolId`, `Comment`, `CompletionToStandard`, `btnSave` ("שמור וסגור").

---

## Attendance Types (Org Ontology)

### From Calendar dropdown (English names):
| Code | Name |
|------|------|
| 0 | work day |
| 120 | work from home |
| 481 | vacation |
| 483 | sickness |
| 485 | family sickness |
| 486 | reserve duty |
| 110 | be'er sheva |
| 489 | mourning |
| 140 | course |
| 130 | work abroad |
| 160 | conference |
| 150 | offsite |
| 501 | Parental leave |

### From Absences API (Hebrew names):
Retrieved via `POST /Hilannetv2/Services/Public/WS/HAbsencesApiapi.asmx/GetInitialData` with `{}` body.
Response `Symbols` array contains `{ Id, Name, DisplayName }` — e.g., `{ Id: "481", Name: "חופשה", DisplayName: "481 - חופשה" }`.

Only vacation (481) and sickness (483) appear in the absences API — the full list comes from the calendar page dropdown.

---

## ASMX JSON API Endpoints

Base: `https://{subdomain}.hilan.co.il/Hilannetv2/Services/Public/WS/`

All **read** endpoints use: `POST`, `Content-Type: application/json`, body `{}`, session cookies.
Write/mutating ASMX endpoints may require additional parameters or H-XSRF-Token — not yet verified.
H-XSRF-Token is present as a hidden field on .aspx pages but currently always empty (not enforced as of April 2026).

| Endpoint | Purpose |
|----------|---------|
| `HGeneralApiapi.asmx/GetAppInitialData` | App bootstrap — menu structure, user info, last login |
| `HGeneralApiapi.asmx/GetItemState` | Current employee state |
| `HAbsencesApiapi.asmx/GetInitialData` | Absences page — symbols, table columns, balances |
| `HEmployeeStripApiapi.asmx/GetData` | Employee strip data for the user's own account |
| `HHomeTasksApiapi.asmx/GetTasksInitialCount` | Error/task count on home page |
| `HHomeTasksApiapi.asmx/GetTasksCount` | Detailed task count |
| `HHomeTasksApiapi.asmx/GetReminderCount` | Reminder count |

### GetAppInitialData Response (key fields):
```json
{
  "Menu": {
    "WelcomeText": "שלום, משתמש",
    "LastLoginText": "חיבורך הקודם למערכת: יום שני 10:22",
    "Modules": [...],
    "Tabs": [...]  // Full navigation structure
  }
}
```

---

## Payslip

### Download PDF
```
GET /Hilannetv2/PersonalFile/PdfPaySlip.aspx?Date=01/{MM}/{YYYY}&UserId={orgId}{username}
```
Returns raw PDF. Validate first 4 bytes = `%PDF`.

In the current implementation, the authenticated payslip path gets `orgId` from bootstrap (`GetData`) rather than scraping it from the homepage again.

### Payslip Range Print
```
GET /Hilannetv2/PersonalFile/PaySlipRangePrint.aspx
```
For printing multiple payslips.

### Modern Payslip Page
```
/Hilannetv2/ng/personal-file/payslip
```
Angular SPA — likely uses ASMX API underneath.

---

## Salary Comparison

```
POST /Hilannetv2/PersonalFile/SalaryAllSummary.aspx
Content-Type: application/x-www-form-urlencoded

__DatePicker_State=01/{MM_start}/{YYYY_start},0,30/{MM_end}/{YYYY_end},0
```
May require `__VIEWSTATE` from initial GET. Response: HTML table with `tr.RSGrid` / `tr.ARSGrid` rows.

---

## Other Observed Pages — In Scope

Pages shaon's shipped commands actually interact with.

### Analyzed Sheet (גיליון מנותח)
```
/Hilannetv2/Attendance/HoursAnalysis.aspx
```
Read-only view of calculated own attendance data. Surfaced as `reports sheet`.

### Correction Log (יומן תיקונים)
```
/Hilannetv2/Attendance/HoursReportLog.aspx
```
Log of attendance corrections the user made on their own account. Surfaced as `reports corrections`.

### Absences (היעדרויות)
```
/Hilannetv2/ng/attendance/absences
```
Angular SPA. Surfaced as `attendance absences` via `HAbsencesApiapi.asmx/GetInitialData` — symbols and balances for the user's own account.

---

## Reports (read-only, self-service)

All at: `/Hilannetv2/Reports/repAttendanceviewerGeneric.aspx?reportName=`

The backing UI is Microsoft SSRS / ReportViewer.

Current shaon behavior:

- `reports show <name>` does a direct GET and parses the first meaningful HTML table from the response
- it does **not** drive the full ReportViewer export flow (`OpType=Export&Format=CSV` / `EXCELOPENXML`)
- bulk date-range export is out of scope for the current `reports show` implementation

Self-service report names accepted by `reports show`:

| reportName | Hebrew | Description |
|------------|--------|-------------|
| `AbsenceReportNEW` | דיווחי נוכחות היעדרות | Own absence reports |
| `AllReportNEW` | נוכחות והיעדרות מרוכז | Combined own attendance & absence |
| `MissingReportNEW` | דיווחי נוכחות חסרים | Missing own attendance reports |
| `ErrorsReportNEW` | דיווחים שגויים | Own attendance error reports |
| `ManualReportingReportNEW` | יומן תיקונים | Own correction log |
| `ManualReportingTotalNEW` | יומן תיקונים מסוכם | Summarized own correction log |
| `AttendanceStatusReportNew2` | סטטוס נוכחות | Own attendance status |
| `CalculateAttendanceData908NEW` | נוכחות מחושבים חודשי | Own monthly calculated |
| `CalculateAttendanceData918NEW` | נוכחות מחושבים יומי | Own daily calculated |

---

## Out of scope

shaon is a single-user self-service tool. Multi-user, manager, approver, roster, employee-directory, file-management, payment-deduction, regulation-exception, and yearly tax-form surfaces of Hilanet exist but are explicitly out of scope. They are not documented here. Adding them would require a project-scope change first; see [CONTRIBUTING.md](CONTRIBUTING.md).
