# `src/company/runtime/`

Parts of `CompanyRuntime` (`../runtime.rs`) split out by seam.

| File | What lives there |
| --- | --- |
| `hive_seat.rs` | The resolve path's fork for approvals a company-hive turn parked (OC-2): `hive_seat_of` reads the `hive-turn:` key, `hold_hive_answer` holds an escalation's answer, and `resume_hive_seat` renders every decision into one release note and asks the brain to release the agent (`Brain::release_hive_agent`), telling the operator when no hive takes it. Tests: `../runtime_hive_seat_tests.rs`. |
