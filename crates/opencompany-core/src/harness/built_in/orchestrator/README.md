# `harness/built_in/orchestrator/`

Child modules of `harness/built_in/orchestrator.rs`.

| File | What lives there |
|------|------------------|
| `insight_reads.rs` | `query_company`'s read outcomes: `RecordRead` (loaded, missing, failed, not wired) for the company record, the `section` helper that logs a failed store read and names the section in the result's `unreadable` list, and the wording each section shows when its read failed. |

Tests are in `harness/built_in/orchestrator_tests_insight_unreadable.rs`.
