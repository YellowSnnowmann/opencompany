# analytics/types

| File | What it is |
|---|---|
| `event.rs` | The `Event` enum and its `name` / `props` / `metered` impls, split out of `../types.rs` (which was at the 750-line cap). Re-exported as `crate::analytics::Event`. |
