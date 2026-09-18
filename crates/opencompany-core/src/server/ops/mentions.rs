//! The mention directory: `GET {scope}/chat/mentionables`.
//!
//! Everything a composer needs to offer an `@` picker — the teammates, the
//! people, the desks, and the broadcast token — in one read, resolved the same
//! way the host resolves a mention it receives.
//!
//! # Why this is not `GET {scope}/users`
//!
//! The user directory is **admin-gated**
//! ([`require_admin`](crate::server::users::admin::require_admin)), and
//! correctly so: it carries login identities, roles, statuses, and invite
//! state, which is administration, not collaboration. But every member has to
//! be able to mention every other member, so mentioning a colleague cannot
//! require being an admin.
//!
//! The answer is a second, much narrower read rather than a relaxation of the
//! first. This route hands out **an id, a label, and the person's chosen face**,
//! and nothing else — no email, no role, no status, no last-seen. The avatar is
//! already a collaboration-facing identity asset: it is shown beside that
//! person's messages to the same members. That is the same discipline
//! [`author_labels`](crate::server::chat_history) already enforces on every
//! message a member reads, so this widens nothing sensitive: a person who has
//! ever posted is already named to their colleagues by exactly this label and
//! face.
//!
//! # Signed-in humans only
//!
//! A machine credential names no person and has no composer, so both the
//! privacy argument and the use case are absent. Same `401` as
//! [`read_state`](super::read_state), and for the same reason.

use crate::server::error::Rejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use serde_json::json;

use crate::AppState;
use crate::runtime::mentions::{EVERYONE_ALIASES, user_label, user_slugs};
use crate::server::error::ApiError;
use crate::server::ops::scope::{ScopedCompany, scoped};

pub fn router() -> Router<AppState> {
    scoped("/chat/mentionables", get(list_mentionables))
}

/// One teammate the composer can offer.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MentionableAgentDto {
    /// The roster id — the authored, typable handle, and what a resolved
    /// mention is stored under.
    id: String,
    /// What to show in the picker. The display name for an operator-added
    /// teammate, the id for a manifest one, which is already human-authored.
    name: String,
    /// The teammate's job title, so two similarly-named teammates are
    /// distinguishable in the list.
    role: String,
}

/// One person the composer can offer.
///
/// **Id, label, and chosen face only.** See the module note: this is
/// deliberately not the admin user record, and must not grow toward it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MentionablePersonDto {
    /// The user id a resolved mention is stored under.
    id: String,
    /// How this person is named to their colleagues — the same label their
    /// messages are attributed with.
    label: String,
    /// The person's collaboration-facing avatar reference, when they chose
    /// one. This carries no login or contact identity and is already shown in
    /// chat alongside the person's authored messages.
    #[serde(skip_serializing_if = "Option::is_none")]
    avatar: Option<String>,
    /// A short typable alias, disambiguated across the company.
    ///
    /// **Not a handle and not stored.** Recomputed on every read, so a rename
    /// can never strand it; it exists so somebody typing fast has something
    /// shorter than a two-word display name to hit.
    slug: String,
}

/// One desk the composer can offer.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MentionableDeskDto {
    /// The desk id a resolved mention is stored under.
    id: String,
    /// The desk's display name.
    name: String,
    /// The teammates a mention of this desk expands to, so the composer can
    /// warn about the blast radius before the message is sent rather than
    /// after.
    member_ids: Vec<String>,
}

/// The broadcast token, described rather than assumed.
///
/// Sent as data so the composer does not hard-code the spellings — the host
/// decides what `@everyone` is called, and a console that disagreed would offer
/// a row that resolves to nothing.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MentionableEveryoneDto {
    /// The canonical spelling to insert.
    label: String,
    /// Every spelling the host accepts, so the picker can match on any of them.
    aliases: Vec<String>,
}

/// `GET {scope}/chat/mentionables` — everything an `@` can name.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MentionablesDto {
    agents: Vec<MentionableAgentDto>,
    people: Vec<MentionablePersonDto>,
    desks: Vec<MentionableDeskDto>,
    everyone: MentionableEveryoneDto,
}

async fn list_mentionables(company: ScopedCompany) -> Result<Json<MentionablesDto>, Rejection> {
    if company.actor.is_none() {
        return Err(unauthorized().into());
    }

    let record = company
        .runtime
        .store()
        .load(company.id())
        .await
        .map_err(|e| ApiError(e).into_response())?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "no such company", "code": "not_found" })),
            )
                .into_response()
        })?;

    // `effective_agents()` rather than the raw manifest: an operator-renamed
    // manifest teammate's stored name lives in an override, and reading the
    // manifest directly would ignore it and advertise the authored id forever.
    let mut agents: Vec<MentionableAgentDto> = record
        .effective_agents()
        .into_iter()
        .map(|a| MentionableAgentDto {
            id: a.id.clone(),
            // A manifest teammate's id is human-authored (`engineer`, `ceo`),
            // so it is already the best label there is for one absent an
            // override.
            name: a.name.clone().unwrap_or(a.id),
            role: a.role,
        })
        .collect();
    agents.extend(
        record
            .overlay_agents
            .iter()
            .filter(|a| !record.is_retired(&a.id))
            .map(|a| MentionableAgentDto {
                id: a.id.clone(),
                name: a.name.clone(),
                role: a.role.clone(),
            }),
    );

    // The caller's own rows are dropped, for both kinds. A self-mention can
    // never survive sending — `normalize` refuses it — so offering a row that
    // names the caller would look pickable and then silently un-chip on
    // reload. An operator token's id matches no user and no agent, so the
    // filter is a no-op for it.
    let self_id = company.actor.as_ref().map(|a| a.id.clone());
    agents.retain(|a| self_id.as_ref().is_none_or(|s| a.id != *s));

    let mut desks: Vec<MentionableDeskDto> = record
        .manifest
        .group_chats
        .iter()
        .map(|c| MentionableDeskDto {
            id: c.id.clone(),
            name: c.name.clone(),
            member_ids: record.effective_desk_members(&c.id),
        })
        .collect();
    desks.extend(record.overlay_desks.iter().map(|d| MentionableDeskDto {
        id: d.id.clone(),
        name: d.name.clone(),
        member_ids: record.effective_desk_members(&d.id),
    }));

    // Sorted by id before the slugs are minted, so the `-2`/`-3` suffix a
    // colliding label gets is stable between two reads. An unsorted list would
    // let two people swap suffixes because a store returned them in a different
    // order, which would move a picker entry under somebody mid-type.
    let mut users = company
        .runtime
        .users()
        .list_users(company.id())
        .await
        .map_err(|e| ApiError(e).into_response())?;
    // `Suspended` is retained only for attribution and is refused on every
    // request (see `UserStatus::Suspended`) — advertising a suspended user
    // here would offer a mention target that can never sign back in to see
    // it, and the same list feeds mention resolution's "live" check below, so
    // an unfiltered list would also let a removed collaborator keep accepting
    // non-quiet direct mentions and `@everyone`.
    users.retain(|u| u.status == crate::ports::users::UserStatus::Active);
    users.sort_by(|a, b| a.id.cmp(&b.id));
    let slugs = user_slugs(&users);
    let people = users
        .iter()
        .zip(slugs)
        .filter(|(u, _)| self_id.as_ref().is_none_or(|s| u.id != *s))
        .map(|(u, slug)| MentionablePersonDto {
            id: u.id.clone(),
            label: user_label(u),
            avatar: u.avatar.clone(),
            slug,
        })
        .collect();

    Ok(Json(MentionablesDto {
        agents,
        people,
        desks,
        everyone: MentionableEveryoneDto {
            label: EVERYONE_ALIASES[0].to_string(),
            aliases: EVERYONE_ALIASES.iter().map(|a| a.to_string()).collect(),
        },
    }))
}

/// The `401` for a caller with no person behind it.
fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({
            "error": "the mention directory is for signed-in people, and this credential names none",
            "code": "unauthorized",
        })),
    )
        .into_response()
}

#[cfg(test)]
#[path = "mentions_tests.rs"]
mod tests;
