//! Compatibility shim: re-exports the fixtures other crate-internal test
//! targets (e.g. `server/ops`'s hosted-fix-error-resolution tests) reached via
//! the old flat `workflow_build::test` path before the split into topical
//! `workflow_build_*_tests` files above.
pub(crate) use super::tests_pass_1::agent_deps;
pub(crate) use super::workflow_build_fixtures_tests::{NativeCopilotModel, NativeStep};
pub(crate) use super::workflow_build_shared_tests::propose_step;
