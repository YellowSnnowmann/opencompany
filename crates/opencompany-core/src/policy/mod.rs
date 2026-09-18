//! Approval policy: the manifest-`[policy]`-driven [`ApprovalGate`].
//!
//! The [`gate`] module implements the default
//! [`ApprovalGate`](crate::ports::ApprovalGate) that evaluates emitted effects
//! against a company's declared policy and holds the in-memory approval queue.
//!
//! The [`always_approve`] module holds the one matcher both approval paths read
//! for `[policy].always_approve` (issue #684). It is always compiled, because
//! the native-effect gate below reads it in the default build while the harness
//! tool policy reads it only under the `openhuman` feature.
//!
//! The [`consequence`] module declares, once, what every tool can reach — the
//! single source both approval questions read ("may this run unattended?" and
//! "may an operator grant it standing?"). It is always compiled, because the
//! standing-grant rule is enforced in the default build (the mint path and the
//! console card) while the tool policy that parks calls compiles only under the
//! `openhuman` feature.

//! The [`floor`] module owns the consequence rule itself — which calls commit
//! the company — read by [`judgement`] and by the shadow measurement above the
//! `policy_hitl_enabled` bypass (issue #2147). It is one implementation with
//! two readers for the reason [`always_approve`] is: this is the second time a
//! single approval rule has needed to be read from two places.
//!
//! The [`judgement`] module answers the question neither of the above asks:
//! which calls should stop for a human *on their own merits*, in the gap the
//! static configuration leaves (issue #338). Always compiled for the same
//! reason [`consequence`] is — it is pure, it has no harness types in it, and
//! keeping it out of the gated build means its tests run in the plain lane.

pub mod always_approve;
pub mod consequence;
pub mod floor;
pub mod gate;
pub mod judgement;

/// Shared `composio_execute` call fixtures (issue #470). Test-only, and
/// deliberately here rather than in any one test module: the key they build
/// their arguments under is the classifier's own constant, which is what stops
/// a fixture and the code under test from drifting apart again.
#[cfg(test)]
pub(crate) mod test_support;

pub use consequence::{Consequence, McpReadSet, Reach, Standing, consequence_of};
pub use gate::{DEFAULT_TTL_MILLIS, ManifestApprovalGate};
pub use judgement::{CallPath, Judgement, StopReason, judge};
