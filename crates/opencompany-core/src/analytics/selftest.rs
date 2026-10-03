//! `opencompany analytics-test`: one deliberate event, proven end to end.
//!
//! The question this answers is the one `/spec.analytics` cannot: "does a real
//! event leave *this* binary, under *this* environment, and does the collector
//! take it?" It resolves the decision exactly as boot does ([`resolve`] over the
//! same [`Deployment::from_env`]), so a launcher that would report reports here
//! and one that would stay silent says so — it never invents a destination.
//!
//! The event is [`Event::AnalyticsSelfTest`], attributed to a throwaway
//! [`OpaqueId::smoke`] (`s_` + 128 random bits). A verification run therefore
//! shows up in the collector under its own profile and cannot be mistaken for,
//! or counted as, a real install.
//!
//! # Exit codes
//!
//! * `0` — the collector accepted the event (`last_send == accepted`).
//! * `1` — the event was queued but the collector did not take it (refused,
//!   unreachable, redirected …), or this build has no transport.
//! * `2` — this process would be silent, so there was nothing to test.
//!
//! A tool that exits `0` while nothing arrived is worse than none: CI uses this
//! to decide whether reporting works, so only a confirmed `accepted` is success.

use std::sync::Arc;

use crate::analytics::config::{Decision, resolve};
use crate::analytics::status::LastSend;
use crate::analytics::types::{Envelope, OpaqueId};
use crate::analytics::{Event, Tracker, openpanel};
use crate::app::config::EnvSource;
use crate::app::deployment::Deployment;
use crate::ports::brain::Cognition;

/// What one self-test run found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelfTestOutcome {
    /// Nothing was sent: the process resolves to silence. Carries the reason.
    Silent(&'static str),
    /// This build compiled without the `analytics` feature, so there is no
    /// transport to test.
    NoTransport,
    /// The event was sent as `profile_id`; `last_send` says how it ended.
    Sent {
        /// The `s_…` profile the event was reported under.
        profile_id: String,
        /// How the send ended.
        last_send: LastSend,
    },
}

impl SelfTestOutcome {
    /// The process exit code this outcome maps to; see the module docs.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Silent(_) => 2,
            Self::NoTransport => 1,
            Self::Sent { last_send, .. } if *last_send == LastSend::Accepted => 0,
            Self::Sent { .. } => 1,
        }
    }
}

/// A fresh 128-bit nonce as 32 lowercase hex digits.
fn random_nonce() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("the OS CSPRNG is unavailable; cannot mint a nonce");
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Runs the self-test against whatever `env` resolves to.
///
/// Builds a fresh tracker rather than reusing a host's, so it needs no running
/// host and cannot disturb one: the only thing it shares with boot is the
/// decision.
pub async fn run(env: &dyn EnvSource) -> SelfTestOutcome {
    let deployment = Deployment::from_env(env);
    let decision = resolve(deployment, env);
    if let Decision::Silent(reason) = &decision {
        return SelfTestOutcome::Silent(reason.as_str());
    }
    if !crate::analytics::BuildFlags::of_this_build().analytics {
        return SelfTestOutcome::NoTransport;
    }

    let profile_id = OpaqueId::smoke(&random_nonce());
    let envelope = Envelope::new(profile_id.clone(), deployment, Cognition::default());
    let tracker: Arc<dyn Tracker> = openpanel::build(&decision, envelope);
    tracker.track(Event::AnalyticsSelfTest {});
    tracker.flush().await;
    let last_send = tracker
        .status()
        .map_or(LastSend::Never, |status| status.last_send);
    SelfTestOutcome::Sent {
        profile_id: profile_id.as_str().to_string(),
        last_send,
    }
}

/// [`run`], printing as the CLI does: the profile id alone on stdout (so it
/// pipes), every diagnostic on stderr. Returns the exit code.
pub async fn run_cli(env: &dyn EnvSource) -> i32 {
    let outcome = run(env).await;
    match &outcome {
        SelfTestOutcome::Silent(reason) => {
            eprintln!("analytics is off in this process, so there is nothing to test: {reason}");
        }
        SelfTestOutcome::NoTransport => eprintln!(
            "this build was compiled without the `analytics` feature, so there is no \
             transport to test"
        ),
        SelfTestOutcome::Sent {
            profile_id,
            last_send,
        } => {
            println!("{profile_id}");
            if *last_send != LastSend::Accepted {
                eprintln!(
                    "the collector did not accept the event (last send: {})",
                    last_send.as_str()
                );
            }
        }
    }
    outcome.exit_code()
}

#[cfg(test)]
#[path = "selftest_tests.rs"]
mod tests;
