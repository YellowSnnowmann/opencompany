//! What a committed utterance's `@names` do: they are journaled on the row,
//! they badge the people they name, and they mint no turn.
//!
//! Driven through the real episode host over a scripted seat runner, so the
//! assertions land on the rows the driver actually writes.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use openhuman_embed::AgentSpec;
use tinyhivemind::speech::Utterance;

use crate::company::runtime::CompanyRuntime;
use crate::harness::openhuman_runtime::{RuntimeBoot, global};
use crate::hive::driver::{
    HiveDispatcher, SeatFailure, SeatOutcome, SeatRunner, SeatTurn, Trigger,
};
use crate::hive::graph::desk_hives;
use crate::hive::test_support::{MemoryLog, operator_message, record};
use crate::ports::events::EventLog;
use crate::ports::types::{CompanyEvent, Mention, MentionTarget, StoredEvent};
use crate::ports::users::{UserRecord, UserRole, UserStatus};

/// One desk of two, with referral on so the negative test has a live edge to
/// prove nothing crosses.
const ONE_DESK: &str = r#"
[company]
name = "Acme"

[[agent]]
id = "ceo"
role = "Chief Executive"

[[agent]]
id = "writer"
role = "Writer"

[[group_chat]]
id = "engineering"
name = "Engineering desk"
description = "How things are built."
members = ["ceo", "writer"]

[group_chat.routing]
round_width = 2

[group_chat.routing.referral]
enabled = true
max_hops = 1
returns = true
"#;

/// Answers from a per-agent script, in order.
struct Script {
    lines: Mutex<HashMap<String, Vec<Utterance>>>,
}

impl Script {
    fn new(lines: &[(&str, Vec<Utterance>)]) -> Arc<Self> {
        Arc::new(Self {
            lines: Mutex::new(
                lines
                    .iter()
                    .map(|(id, says)| ((*id).to_string(), says.clone()))
                    .collect(),
            ),
        })
    }
}

#[async_trait]
impl SeatRunner for Script {
    async fn run_seat(&self, seat: SeatTurn) -> std::result::Result<SeatOutcome, SeatFailure> {
        seat.bracket_started().await;
        let next = self
            .lines
            .lock()
            .unwrap()
            .get_mut(&seat.agent_id)
            .and_then(|says| (!says.is_empty()).then(|| says.remove(0)));
        let outcome = Ok(SeatOutcome {
            reply: String::new(),
            utterances: vec![next.unwrap_or(Utterance::CompleteEpisode {
                message: "done".into(),
            })],
            ..Default::default()
        });
        seat.bracket_settled(&outcome).await;
        outcome
    }
}

fn post(text: &str) -> Utterance {
    Utterance::Post {
        message: text.into(),
    }
}

fn complete(text: &str) -> Utterance {
    Utterance::CompleteEpisode {
        message: text.into(),
    }
}

/// A live runtime over a temp home, one human collaborator seeded, and an
/// episode host whose mention seam is that runtime's.
///
/// The host's company record is parsed from the same manifest the runtime was
/// built with, so the directory the seam resolves against is the roster the
/// desks are seated from.
async fn host(
    script: Arc<Script>,
) -> (
    HiveDispatcher,
    Arc<MemoryLog>,
    Arc<CompanyRuntime>,
    String,
    tempfile::TempDir,
) {
    let home = tempfile::Builder::new()
        .prefix("opencompany-hive-mentions-")
        .tempdir()
        .expect("tempdir");
    let manifest: crate::company::CompanyManifest =
        toml::from_str(ONE_DESK).expect("test manifest parses");
    let runtime = Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
            .with_id(MemoryLog::company())
            .build()
            .await
            .expect("runtime"),
    );

    let person = crate::ports::generate_id();
    let now = crate::ports::now_millis();
    runtime
        .users()
        .upsert_user(
            runtime.id(),
            &UserRecord {
                id: person.clone(),
                email: "dana@example.test".to_string(),
                display_name: Some("Dana".to_string()),
                avatar: None,
                role: UserRole::Member,
                status: UserStatus::Active,
                password_hash: None,
                must_change_password: false,
                created_at_millis: now,
                last_seen_at_millis: None,
                updated_at_millis: now,
            },
        )
        .await
        .expect("seed the person");

    let embedded = global(RuntimeBoot::ephemeral()).await.expect("runtime");
    let salt = uuid::Uuid::new_v4().simple().to_string();
    let agents: HashMap<String, openhuman_embed::Agent> = ["ceo", "writer"]
        .into_iter()
        .map(|id| {
            let agent = embedded
                .agent(AgentSpec::new(format!("hive-mentions-{id}-{}", &salt[..8])))
                .expect("agent");
            (id.to_string(), agent)
        })
        .collect();
    let record = Arc::new(record(ONE_DESK));
    let (hives, errors) = desk_hives(&record, 1, &|id| agents.get(id).cloned());
    assert!(errors.is_empty(), "{errors:?}");

    let log = Arc::new(MemoryLog::default());
    (
        HiveDispatcher {
            record,
            events: log.clone(),
            hives,
            router: None,
            seats: script,
            runs: None,
            mentions: Some(runtime.mention_seam()),
        },
        log,
        runtime,
        person,
        home,
    )
}

/// Opens an episode on the desk with `text` and runs it to completion.
async fn run_episode(host: &HiveDispatcher, log: &MemoryLog, text: &str) {
    let seq = log
        .append(
            &MemoryLog::company(),
            operator_message("engineering", text, None),
        )
        .await
        .unwrap();
    host.run_desk_message(
        "engineering",
        Trigger {
            seq,
            text: text.into(),
            parent: None,
            mentions: Vec::new(),
            hop: 0,
            origin: None,
            referred_from: None,
        },
    )
    .await
    .expect("the episode runs");
}

/// Every `AgentReply` the run journaled, as `(author, text, mentions)`.
fn replies(rows: &[StoredEvent]) -> Vec<(String, String, Vec<Mention>)> {
    rows.iter()
        .filter_map(|stored| match &stored.event {
            CompanyEvent::AgentReply {
                agent_id,
                text,
                mentions,
                ..
            } => Some((agent_id.clone(), text.clone(), mentions.clone())),
            _ => None,
        })
        .collect()
}

/// The bug: a committed utterance naming a teammate journaled
/// `mentions: []`, so 63 of 105 replies on a live journal named somebody and
/// recorded nobody.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_committed_utterance_journals_the_mentions_it_names() {
    let script = Script::new(&[
        ("ceo", vec![post("@writer take the draft."), complete("ok")]),
        ("writer", vec![complete("on it")]),
    ]);
    let (host, log, _runtime, _person, _home) = host(script).await;
    run_episode(&host, &log, "Plan the login page.").await;

    let named = replies(&log.rows())
        .into_iter()
        .find(|(_, text, _)| text.contains("@writer"))
        .expect("the utterance naming the writer was journaled");
    assert_eq!(
        named.2.len(),
        1,
        "the row has to carry the teammate it names, not an empty list: {:?}",
        named.2
    );
    assert_eq!(
        named.2[0].target,
        MentionTarget::Agent {
            id: "writer".to_string()
        }
    );
    assert_eq!(named.2[0].text, "@writer");
}

/// The hive twin of `a_mention_in_an_agent_reply_notifies_the_person_it_names`
/// (`server/operator_test_group_14.rs`): the orchestrator path files this row
/// and the desk path did not, for the same `@` in the same company.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_person_named_in_a_desk_reply_is_notified() {
    let script = Script::new(&[
        ("ceo", vec![post("@Dana can you confirm?"), complete("ok")]),
        ("writer", vec![complete("nothing from me")]),
    ]);
    let (host, log, runtime, person, _home) = host(script).await;
    run_episode(&host, &log, "Plan the login page.").await;

    let named = replies(&log.rows())
        .into_iter()
        .find(|(_, text, _)| text.contains("@Dana"))
        .expect("the utterance naming Dana was journaled");
    assert_eq!(
        named.2.first().map(|m| &m.target),
        Some(&MentionTarget::User { id: person.clone() }),
        "the person has to resolve through the same directory the operator \
         path uses: {:?}",
        named.2
    );

    let notes = runtime
        .notifications()
        .list(runtime.id(), &person)
        .await
        .expect("notifications read");
    assert_eq!(notes.len(), 1, "one row, not one per recipient: {notes:?}");
    assert_eq!(notes[0].notification.kind, "mention");
    assert_eq!(
        notes[0].notification.audience.as_deref(),
        Some(std::slice::from_ref(&person)),
        "the audience has to be the person the reply named"
    );
}

/// A teammate this desk already seats is named. The round runs every seat
/// every round, so that teammate is already holding the whole conversation;
/// minting a turn here would give it two per round.
///
/// This is the fuse the `AgentReply::mentions` doc claims, made enforceable:
/// the mention is recorded and it moves nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_same_desk_mention_records_but_mints_no_turn_and_no_child_cycle() {
    let script = Script::new(&[
        ("ceo", vec![post("@writer take the draft."), complete("ok")]),
        ("writer", vec![complete("on it")]),
    ]);
    let (host, log, _runtime, _person, _home) = host(script).await;
    run_episode(&host, &log, "Plan the login page.").await;

    let named = replies(&log.rows())
        .into_iter()
        .find(|(_, text, _)| text.contains("@writer"))
        .expect("the utterance naming the writer was journaled");
    assert_eq!(named.2.len(), 1, "recorded");

    let kinds: Vec<&'static str> = log.rows().iter().map(|row| row.event.kind()).collect();
    assert!(
        !kinds.contains(&"ReferralEnqueued"),
        "a same-desk mention must enqueue no referral: {kinds:?}"
    );
    assert!(
        !kinds.contains(&"TaskDispatched"),
        "a same-desk mention must mint no child cycle: {kinds:?}"
    );

    // The invariant the round commits under: one settled utterance per seat
    // per round. A turn minted by the mention would seat the writer twice in
    // the round that named it.
    let mut seated: Vec<(u64, String)> = log
        .rows()
        .iter()
        .filter_map(|row| match &row.event {
            CompanyEvent::AgentReply {
                agent_id,
                episode: Some(episode),
                ..
            } => Some((episode.revision, agent_id.clone())),
            _ => None,
        })
        .collect();
    let seated_len = seated.len();
    seated.sort();
    seated.dedup();
    assert_eq!(
        seated.len(),
        seated_len,
        "a seat settled twice in one round: {seated:?}"
    );
}

/// The two halves of a mention must agree: the set stored on the row and the
/// set the referral decision acts on are one resolution, not two.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_stored_mentions_are_the_ones_the_referral_decision_reads() {
    let script = Script::new(&[
        (
            "ceo",
            vec![post("@writer and @Dana, please."), complete("ok")],
        ),
        ("writer", vec![complete("on it")]),
    ]);
    let (host, log, _runtime, person, _home) = host(script).await;
    run_episode(&host, &log, "Plan the login page.").await;

    let named = replies(&log.rows())
        .into_iter()
        .find(|(_, text, _)| text.contains("@writer"))
        .expect("the utterance was journaled");
    let stored: Vec<_> = named.2.iter().map(|m| m.target.clone()).collect();
    assert_eq!(
        stored,
        vec![
            MentionTarget::Agent {
                id: "writer".to_string()
            },
            MentionTarget::User { id: person },
        ],
        "both targets, in authored order"
    );

    // What `refer` reads is these same rows put through the library's shape,
    // so the targets it judges are the targets that were recorded.
    let carried: Vec<_> = named
        .2
        .iter()
        .map(crate::hive::dispatch::tinyhivemind_mention)
        .map(|m| m.target)
        .collect();
    assert_eq!(
        carried,
        vec![
            tinyhivemind_core::mention::MentionTarget::Agent {
                id: "writer".to_string()
            },
            tinyhivemind_core::mention::MentionTarget::Person {
                id: stored
                    .iter()
                    .find_map(|t| match t {
                        MentionTarget::User { id } => Some(id.clone()),
                        _ => None,
                    })
                    .expect("the person target"),
            },
        ],
    );
}

/// The seam resolves nothing when it is absent, and the round still commits —
/// the state every driver test and every pre-seam host is in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_host_with_no_seam_journals_a_reply_with_no_mentions() {
    let script = Script::new(&[
        ("ceo", vec![post("@writer take the draft."), complete("ok")]),
        ("writer", vec![complete("on it")]),
    ]);
    let (mut host, log, _runtime, _person, _home) = host(script).await;
    host.mentions = None;
    run_episode(&host, &log, "Plan the login page.").await;

    let named = replies(&log.rows())
        .into_iter()
        .find(|(_, text, _)| text.contains("@writer"))
        .expect("the utterance was still journaled");
    assert!(named.2.is_empty(), "no seam, no mentions: {:?}", named.2);
}
