# `harness/pages_tools/`

Child modules of `harness/pages_tools.rs`, the agent tools over `pages/<slug>/`.

| File | What lives there |
|------|------------------|
| `manifest_read.rs` | `ManifestRead` — the outcome of reading a page's `page.toml` (found, absent, unparseable, unreadable) — with `CompanyPages::read_manifest` and the `pages_list` line, `pages_read` header and `pages_write` refusal for a manifest whose contents are unknown. Its tests are `harness/pages_tools_manifest_tests.rs`. |
