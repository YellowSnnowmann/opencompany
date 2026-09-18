//! Machines that execute this host's work, on the operator's own hardware.
//!
//! A **runner** is a process — usually the desktop app — that holds its own
//! keypair, dials *out* to a host, advertises the coding harnesses installed on
//! its machine, and can be handed a company's task to run there against real
//! files and real local credentials.
//!
//! ## The shape is block/buzz's, and the axioms are worth restating
//!
//! Buzz's remote-agent design gets several things right that are easy to get
//! wrong, and this lane adopts them deliberately:
//!
//! - **The runner dials out.** A host never opens a connection to a runner.
//!   That is not merely convenient for NAT — it means there is no inbound
//!   surface on someone's laptop for a host to be trusted with.
//! - **No management channel.** The control protocol contains no status query,
//!   no exec, no kill. Everything a host wants to know it learns from what the
//!   runner says; everything it wants done it asks for in-band.
//! - **Presence is status.** A runner heartbeats; a host that stops hearing one
//!   stops scheduling to it. There is no "are you alive?" call, because the
//!   answer to that question is only ever as fresh as the last heartbeat
//!   anyway.
//! - **Identity fails closed.** A runner with no key does not start.
//! - **At most one live instance per scope**, so two copies of a desktop cannot
//!   both take the same company's work.
//!
//! ## What is deliberately *not* claimed
//!
//! [`RunnerRegistry`] is in-memory and per process. Two hosts behind a load
//! balancer can each believe they hold a scope, and a restart forgets
//! everything. This is not distributed mutual exclusion and must not be
//! described as such — the guarantee is "at most one per host process".

pub mod attest;
pub mod dispatch;
pub mod registry;

pub use attest::{OwnerAttestation, RunnerHello, verify_hello};
pub use dispatch::{RunnerDispatch, RunnerLink, SessionMap};
pub use registry::{RunnerRegistry, RunnerStatus};
