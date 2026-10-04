//! Who starts an operator message on a hive.
//!
//! The Coordinator conducts an episode from whichever members a host message
//! names as its `starters`; everyone else on the hive reads the message and
//! may be drawn in by the conductor's own rules. Choosing the starters stays a
//! host decision, because the two signals it reads are host facts:
//!
//! 1. **Mention.** A message that names teammates (or a desk, or `@everyone`)
//!    starts exactly the named members of this hive — naming somebody is the
//!    strongest address there is.
//! 2. **Jev.** An unaddressed desk message is put to System One through
//!    TinyHiveMind's `route_message`, with the desk's members as candidates
//!    and the desk's frozen `RoutingPolicy`; an accepted plan starts its
//!    primary and any invited members.
//! 3. **Default.** Otherwise — no credential, a refused or unclear plan,
//!    `#general` — the hive's default responder starts alone: the desk's
//!    first roster member, or the caller's fallback (the orchestrator).
//!
//! Every starter is checked against the hive's membership: a starter that is
//! not a reader of the message is refused by the Coordinator, and a refused
//! send loses the operator's line.

use tinyhivemind_core::embed::{
    ConversationKind, ConversationRef, RouteCandidate, Router, RoutingPlan, RoutingRequest,
    RoutingSource, route_message,
};

use crate::ports::general_channel::GENERAL_CHANNEL_ID;
use crate::ports::types::{CompanyRecord, Mention, MentionTarget};
use crate::runtime::delegation_tools::{
    desk_default_responder, tinyhivemind_desks, tinyhivemind_roster,
};

/// How a message's starters were chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StarterRoute {
    /// The message named them.
    Mention,
    /// System One routed it.
    Jev,
    /// The hive's default responder.
    Default,
}

impl StarterRoute {
    /// The wire word journaled on `HiveAccepted.route`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mention => "mention",
            Self::Jev => "jev",
            Self::Default => "default",
        }
    }
}

/// The members a hive message starts, by manifest id, and how they were
/// chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Starters {
    /// Manifest agent ids, in order, each a member of the hive.
    pub members: Vec<String>,
    /// How they were chosen.
    pub route: StarterRoute,
}

/// One host mention in the library's shape. The host's `User` is the
/// library's `Person`; everything else is the same target under another name.
#[must_use]
pub fn tinyhivemind_mention(mention: &Mention) -> tinyhivemind_core::mention::Mention {
    use tinyhivemind_core::mention::MentionTarget as Target;
    let target = match &mention.target {
        MentionTarget::Agent { id } => Target::Agent { id: id.clone() },
        MentionTarget::User { id } => Target::Person { id: id.clone() },
        MentionTarget::Desk { id } => Target::Desk { id: id.clone() },
        MentionTarget::Everyone => Target::Everyone,
    };
    tinyhivemind_core::mention::Mention {
        target,
        text: mention.text.clone(),
        offset: mention.offset,
        quiet: mention.quiet,
    }
}

/// The hive's members, by manifest id, in desk order: roster agents only.
#[must_use]
pub fn hive_members(record: &CompanyRecord, hive_id: &str) -> Vec<String> {
    record
        .effective_desk_members(hive_id)
        .into_iter()
        .filter(|member| record.is_roster_agent(member))
        .collect()
}

/// The members of `hive_id` the message named — teammates directly, a desk
/// or `@everyone` expanded against the addressed hive — in reading order.
#[must_use]
pub fn mentioned_starters(record: &CompanyRecord, hive_id: &str, mentions: &[Mention]) -> Vec<String> {
    if mentions.is_empty() {
        return Vec::new();
    }
    let members = tinyhivemind_roster(record);
    let retired = record.overlay_retired_agents.clone();
    let roster = tinyhivemind_core::roster::Roster::new(&members, &[], &retired);
    let snapshots = tinyhivemind_desks(record);
    let desks = snapshots.set();
    let mentions: Vec<tinyhivemind_core::mention::Mention> =
        mentions.iter().map(tinyhivemind_mention).collect();
    let mut named: Vec<String> = Vec::new();
    if let Some(direct) = tinyhivemind_core::mention::direct_responder(&mentions, &roster) {
        named.push(direct.to_string());
    }
    for member in tinyhivemind_core::mention::mentioned_members(
        &mentions,
        Some(hive_id),
        named.first().map(String::as_str),
        &roster,
        &desks,
    ) {
        if !named.contains(&member) {
            named.push(member);
        }
    }
    let on_hive = hive_members(record, hive_id);
    named.retain(|member| on_hive.contains(member));
    named
}

/// The hive's default responder: its first roster member, else `fallback`
/// when that is a member, else whoever is first.
#[must_use]
pub fn default_starter(record: &CompanyRecord, hive_id: &str, fallback: &str) -> Option<String> {
    let members = hive_members(record, hive_id);
    if hive_id == GENERAL_CHANNEL_ID && members.iter().any(|member| member == fallback) {
        return Some(fallback.to_string());
    }
    desk_default_responder(record, hive_id)
        .filter(|lead| members.contains(lead))
        .or_else(|| {
            members
                .iter()
                .find(|member| member.as_str() == fallback)
                .cloned()
        })
        .or_else(|| members.first().cloned())
}

/// The candidates a Jev route chooses among: the hive's members with their
/// profile.
#[must_use]
pub fn candidates(record: &CompanyRecord, hive_id: &str) -> Vec<RouteCandidate> {
    let agents = record.effective_agents();
    hive_members(record, hive_id)
        .into_iter()
        .map(|member| {
            let profile = agents.iter().find(|agent| agent.id == member);
            RouteCandidate {
                label: profile
                    .and_then(|agent| agent.name.clone())
                    .unwrap_or_else(|| member.clone()),
                role: profile.map(|agent| agent.role.clone()),
                description: profile.and_then(|agent| agent.description.clone()),
                capabilities: Vec::new(),
                learned_topics: Vec::new(),
                available: true,
                id: member,
            }
        })
        .collect()
}

/// The starters of one operator message on `hive_id`.
///
/// `router` is the Jev router when a credential resolved one, `fallback` the
/// teammate who answers when nothing else does (the orchestrator).
pub async fn choose(
    record: &CompanyRecord,
    hive_id: &str,
    text: &str,
    mentions: &[Mention],
    router: Option<&dyn Router>,
    fallback: &str,
) -> Starters {
    let named = mentioned_starters(record, hive_id, mentions);
    if !named.is_empty() {
        return Starters {
            members: named,
            route: StarterRoute::Mention,
        };
    }
    let default = default_starter(record, hive_id, fallback);
    let fallback_members = || default.iter().cloned().collect::<Vec<_>>();
    let Some(primary) = default.as_deref() else {
        return Starters {
            members: Vec::new(),
            route: StarterRoute::Default,
        };
    };
    if hive_id == GENERAL_CHANNEL_ID || router.is_none() {
        return Starters {
            members: fallback_members(),
            route: StarterRoute::Default,
        };
    }
    let request = RoutingRequest {
        message: text.to_string(),
        source: RoutingSource::DeskMessage,
        conversation: ConversationRef {
            id: hive_id.to_string(),
            kind: ConversationKind::Desk,
            thread_root: None,
        },
        desk_purpose: record
            .manifest
            .group_chats
            .iter()
            .find(|chat| chat.id == hive_id)
            .map(|chat| chat.name.clone()),
        thread_context: Vec::new(),
        candidates: candidates(record, hive_id),
        roster_version: 0,
        policy: crate::hive::routing::desk_routing(record, hive_id).policy(),
    };
    let plan = route_message(router, None, &request, None, primary).await;
    let members = hive_members(record, hive_id);
    let routed: Vec<String> = match plan {
        RoutingPlan::One { responder_id, .. } => vec![responder_id],
        RoutingPlan::Hive {
            primary_id,
            invited_ids,
            ..
        } => std::iter::once(primary_id).chain(invited_ids).collect(),
        RoutingPlan::Clarify { .. } | RoutingPlan::Fallback { .. } => Vec::new(),
    };
    let mut accepted: Vec<String> = Vec::new();
    for member in routed {
        if members.contains(&member) && !accepted.contains(&member) {
            accepted.push(member);
        }
    }
    if accepted.is_empty() {
        Starters {
            members: fallback_members(),
            route: StarterRoute::Default,
        }
    } else {
        Starters {
            members: accepted,
            route: StarterRoute::Jev,
        }
    }
}

/// The Jev router this host routes a company's desks with, or `None` when no
/// credential resolves one.
///
/// # Which key, in what order
///
/// 1. `OPENCOMPANY_JEV_KEY`, because routing and inference are not always the
///    same vendor and an operator who names a routing vendor means it.
/// 2. The company's own account key (`tinyhumans/key`), what a console
///    sign-in stores.
/// 3. The inherited environment ladder inside `jev::jev_router`.
///
/// Read live on every message rather than cached, so a company that signs in
/// while the host is up routes through Jev on its next message.
#[cfg(feature = "openhuman")]
pub async fn host_router(
    company: &crate::ports::types::CompanyId,
    secrets: Option<&std::sync::Arc<dyn crate::ports::SecretStore>>,
) -> Option<std::sync::Arc<dyn Router>> {
    let env = &crate::app::config::ProcessEnv;
    let credential = crate::app::config::EnvSource::get(env, crate::hive::jev::JEV_KEY_ENV)
        .map(crate::company::Credential::from_value)
        .unwrap_or_default();
    let credential = if credential.configured() {
        credential
    } else if let Some(secrets) = secrets {
        match crate::company::company_key::load(company, secrets.as_ref()).await {
            Ok(key) => key,
            Err(error) => {
                tracing::warn!(
                    %error,
                    "[hive] the company's account key could not be read; routing by mention and lead"
                );
                return None;
            }
        }
    } else {
        credential
    };
    let resolved = if credential.configured() {
        crate::hive::jev::jev_router_from(env, credential)
    } else {
        crate::hive::jev::jev_router(env, None)
    };
    match resolved {
        Ok(Some(router)) => Some(std::sync::Arc::new(router)),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(%error, "[hive] the Jev router is misconfigured; routing by mention and lead");
            None
        }
    }
}

#[cfg(test)]
#[path = "route_tests.rs"]
mod tests;
