# Test Fixtures

This directory is reserved for captured Hilan responses used by parser and
protocol tests.

## Conventions

- Keep fixtures grouped by feature area, for example `attendance/`,
  `reports/`, or `salary/`.
- Prefer raw upstream responses over hand-edited snippets so tests exercise the
  real protocol shape.
- Store a short metadata note alongside each fixture set that records:
  command or endpoint, capture date, authenticated page path, and any manual
  redactions applied.
- Sanitize secrets before committing: usernames, employee IDs, organization
  IDs, cookies, tokens, comments, and any payroll amounts that should not be
  public.
- When a test depends on a specific bug shape, keep the fixture filename
  descriptive, for example `calendar-april-2026-missing-day.html`.

## Suggested Layout

- `attendance/` for calendar and error-wizard HTML
- `reports/` for parsed table pages such as analyzed sheets or correction logs
- `ontology/` for attendance-type snapshots
- `salary/` for salary summary HTML

Keep fixture additions minimal and purpose-driven: add the smallest real
capture that reproduces the parsing or protocol behavior under test.
