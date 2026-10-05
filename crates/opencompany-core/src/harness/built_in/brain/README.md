# `src/harness/built_in/brain/`

Parts of `HarnessBrain` (`../brain.rs`) split out by seam.

| File | What lives there |
| --- | --- |
| `hive_chat.rs` | The operator path into the company hive (OC-2): `send_to_hive` hands an operator line to the Coordinator with `send_as_host` (message id `op:{seq}`, starters from `hive::route::choose`), retries an `InvalidThread` at the top level, treats `MessageConflict` as already accepted, turns `InboxFull` into a system line, and journals `HiveAccepted`; `hive_seat_of` tells a hive turn's parked approval from a cycle's. |
