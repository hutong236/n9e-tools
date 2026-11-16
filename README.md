# n9e-tools

Rust + Axum Web tool for bulk importing Nightingale v8 alert rules from Excel.

## Planned import behavior

- Read and associate 11 Excel worksheets.
- Select 54 red-highlighted rows.
- Preserve original PromQL expressions and durations.
- Skip rules with duplicate names.
- Create imported rules disabled by default.
- Validate and preview before writing and report per-rule outcomes.

Status: development in progress. Production Nightingale v8 API integration and build/tests are pending.
