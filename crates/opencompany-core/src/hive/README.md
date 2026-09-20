# `src/hive/`

Hive desks: one `OpenHumanHive` + `CompletionDriver` per `[[group_chat]]`
over the process-wide `openhuman_embed::Runtime`. The spec is
`docs/spec/runtime/hive.md`; this folder is its implementation, one seam per
file.

| File | What lives there |
| --- | --- |
| `mod.rs` | Index only. |
| `jev.rs` / `jev_tests.rs` | `TinyHumansSystemOne`, the `SystemOneTransport` over the TinyHumans System One proxy (`OPENCOMPANY_JEV_URL`), and `jev_router`, which answers `None` without a TinyHumans key so the driver routes by lead and mention. Gated on `openhuman`. |
