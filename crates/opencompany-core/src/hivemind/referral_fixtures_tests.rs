//! Tests for cross-desk referral: what crosses, what does not, and what a
//! host owes the library.
//!
//! Everything here is scripted through [`HiveTurnRunner`] and
//! [`HiveReferralRunner`] — no model, no store, no provider — because the
//! properties under test are this host's, not a model's. Whether a room asks a
//! good question is a model's business; whether the answer lands on the right
//! desk, under an author that cannot be counted as a supporter, and only when
//! the desk opted in, is entirely ours.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use super::moves_fixtures_tests::Runner;
use super::test::{MemoryLog, desk_of, record};
use super::*;
use crate::Result;
use crate::ports::events::EventLog;
use crate::ports::types::{CompanyEvent, EventSeq};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A two-desk company: `eng` deliberates, `platform` is the desk it may ask.
///
/// `platform` has two members on purpose, so a `@#platform` mention has a real
/// choice to make and the test asserts the library's pick rather than the only
/// candidate there was.
/// The same two desks, with `planner` seated on BOTH.
///
/// A seat is offered only the peer desks it is a member of, because membership
/// is what makes a crossing's answer legible — `agent_channels` gives a seat
/// the desks it sits on, so a question put to one of those lands somewhere the
/// asker can read, while a crossing to a desk it is not on returns one report
/// line and nothing behind it. [`two_desks`] therefore offers `planner`
/// nothing, which is correct and useless for testing the block's contents.
pub(super) fn two_desks_sharing_a_seat(hive: &str) -> String {
    two_desks(hive).replace(
        "members = [\"sre\", \"dba\"]",
        "members = [\"sre\", \"dba\", \"planner\"]",
    )
}

pub(super) fn two_desks(hive: &str) -> String {
    format!(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"planner\"\nrole = \"Planner\"\n\
         [[agent]]\nid = \"scout\"\nrole = \"Scout\"\n\
         [[agent]]\nid = \"critic\"\nrole = \"Critic\"\n\
         [[agent]]\nid = \"sre\"\nrole = \"SRE\"\n\
         [[agent]]\nid = \"dba\"\nrole = \"DBA\"\n\
         [[group_chat]]\nid = \"eng\"\nname = \"Engineering\"\n\
         description = \"Ship the rollout\"\n\
         members = [\"planner\", \"scout\", \"critic\"]\n\
         {hive}\n\
         [[group_chat]]\nid = \"platform\"\nname = \"Platform\"\n\
         description = \"Owns the database and the edge\"\n\
         members = [\"sre\", \"dba\"]\n"
    )
}

/// The operator's message on `eng`, and the watermark the episode opens on.
pub(super) async fn open(log: &MemoryLog) -> EventSeq {
    log.append(
        &MemoryLog::company(),
        CompanyEvent::OperatorMessage {
            text: "Decide the rollout.".into(),
            by: None,
            chat: Some("eng".into()),
            parent: None,
            deliverable: None,
            mentions: Vec::new(),
            attachments: Vec::new(),
        },
    )
    .await
    .expect("the journal accepts the operator's message")
}

/// A far desk that answers every question with one fixed line, and records
/// every question it was asked.
pub(super) struct FarDesk {
    answer: String,
    asked: Mutex<Vec<(String, String, String)>>,
    broken: bool,
    falters_after: Option<usize>,
    silent_after: Option<usize>,
}

impl FarDesk {
    pub(super) fn answering(answer: &str) -> Self {
        Self {
            answer: answer.to_owned(),
            asked: Mutex::new(Vec::new()),
            broken: false,
            falters_after: None,
            silent_after: None,
        }
    }

    pub(super) fn broken() -> Self {
        Self {
            answer: String::new(),
            asked: Mutex::new(Vec::new()),
            broken: true,
            falters_after: None,
            silent_after: None,
        }
    }

    /// Answers normally until `turns` have run, then fails every later turn.
    pub(super) fn failing_after(turns: usize, answer: &str) -> Self {
        Self {
            falters_after: Some(turns),
            ..Self::answering(answer)
        }
    }

    /// Answers normally until `turns` have run, then returns an empty reply.
    pub(super) fn silent_after(turns: usize, answer: &str) -> Self {
        Self {
            silent_after: Some(turns),
            ..Self::answering(answer)
        }
    }

    /// Every `(desk, agent, prompt)` this runner was handed, in order.
    pub(super) fn asked(&self) -> Vec<(String, String, String)> {
        self.asked.lock().expect("poisoned").clone()
    }
}

#[async_trait]
impl HiveReferralRunner for FarDesk {
    async fn refer(&self, desk_id: &str, agent_id: &str, prompt: &str) -> Result<String> {
        let ran = {
            let mut asked = self.asked.lock().expect("poisoned");
            asked.push((desk_id.to_owned(), agent_id.to_owned(), prompt.to_owned()));
            asked.len()
        };
        if self.silent_after.is_some_and(|after| ran > after) {
            return Ok("   ".to_owned());
        }
        if self.falters_after.is_some_and(|after| ran > after) || self.broken {
            return Err(crate::error::OpenCompanyError::Config(
                "turn for 'sre' hit the harness's per-turn wall-clock ceiling after 10m 00s"
                    .to_owned(),
            ));
        }
        Ok(self.answer.clone())
    }
}

/// A far desk that answers as a **room**: `deliberate` returns a conclusion, so
/// the single-seat `refer` seam is never reached.
///
/// Modelled on how the real host behaves rather than on what is convenient to
/// assert: `deliberate` yielding `Some` is a desk that convened, `None` is a
/// desk that cannot hold a room at all (no `[hive]` block, one member, quorum
/// out of reach), and `Err` is a room that was stood up and then broke.
pub(super) struct Room {
    conclusion: Option<String>,
    broken: bool,
    /// Every `(desk, asker, prompt)` handed to `deliberate`, in order.
    convened: Mutex<Vec<(String, String, String)>>,
    /// And every `(desk, agent, prompt)` that fell through to one seat.
    fell_back: Mutex<Vec<(String, String, String)>>,
}

impl Room {
    pub(super) fn answering(conclusion: &str) -> Self {
        Self {
            conclusion: Some(conclusion.to_owned()),
            broken: false,
            convened: Mutex::new(Vec::new()),
            fell_back: Mutex::new(Vec::new()),
        }
    }

    /// A desk that cannot deliberate, so the crossing must ask its seat.
    pub(super) fn cannot() -> Self {
        Self {
            conclusion: None,
            broken: false,
            convened: Mutex::new(Vec::new()),
            fell_back: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn broken() -> Self {
        Self {
            conclusion: None,
            broken: true,
            convened: Mutex::new(Vec::new()),
            fell_back: Mutex::new(Vec::new()),
        }
    }

    pub(super) fn convened(&self) -> Vec<(String, String, String)> {
        self.convened.lock().expect("poisoned").clone()
    }

    pub(super) fn fell_back(&self) -> Vec<(String, String, String)> {
        self.fell_back.lock().expect("poisoned").clone()
    }
}

#[async_trait]
impl HiveReferralRunner for Room {
    async fn refer(&self, desk_id: &str, agent_id: &str, prompt: &str) -> Result<String> {
        self.fell_back.lock().expect("poisoned").push((
            desk_id.to_owned(),
            agent_id.to_owned(),
            prompt.to_owned(),
        ));
        Ok("one seat's opinion".to_owned())
    }

    async fn deliberate(&self, desk_id: &str, asker: &str, prompt: &str) -> Result<Option<String>> {
        self.convened.lock().expect("poisoned").push((
            desk_id.to_owned(),
            asker.to_owned(),
            prompt.to_owned(),
        ));
        if self.broken {
            return Err(crate::error::OpenCompanyError::Config(
                "the referred desk's episode could not be stood up".to_owned(),
            ));
        }
        Ok(self.conclusion.clone())
    }
}

/// The `hive` block a desk that refers is declared with.
pub(super) const REFERRING: &str = "hive = { turn_budget = 6, quorum = 2, blind_round = false, \
                         referral = { enabled = true } }";

/// The same desk with referral left unsaid, which is every desk by default.
pub(super) const PLAIN: &str = "hive = { turn_budget = 6, quorum = 2, blind_round = false }";

/// Open a driver over `eng`, with or without the federation wired.
pub(super) async fn run(
    hive: &str,
    lines: &[(&str, &str)],
    far: Option<&FarDesk>,
) -> (Arc<MemoryLog>, EpisodeOutcome) {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let manifest = two_desks(hive);
    let desk = desk_of(&manifest, "eng").expect("a room");
    let runner = Runner::new(lines);
    let federation = desk_federation(&record(&manifest), &desk);
    let mut driver = EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    );
    if let (Some(federation), Some(far)) = (federation, far) {
        driver = driver.with_federation(federation, far);
    }
    let outcome = driver.run(trigger).await.expect("the episode runs");
    (log, outcome)
}

/// The same, over any referral runner — `run` is fixed to [`FarDesk`].
pub(super) async fn run_with(
    hive: &str,
    lines: &[(&str, &str)],
    far: &dyn HiveReferralRunner,
) -> (Arc<MemoryLog>, EpisodeOutcome) {
    let log = Arc::new(MemoryLog::default());
    let trigger = open(&log).await;
    let manifest = two_desks(hive);
    let desk = desk_of(&manifest, "eng").expect("a room");
    let runner = Runner::new(lines);
    let federation = desk_federation(&record(&manifest), &desk).expect("a federation");
    let outcome = EpisodeDriver::new(
        MemoryLog::company(),
        desk,
        Arc::clone(&log) as Arc<dyn EventLog>,
        &runner,
        "Decide the rollout.",
    )
    .with_federation(federation, far)
    .run(trigger)
    .await
    .expect("the episode runs");
    (log, outcome)
}

// ---------------------------------------------------------------------------
// The snapshot
// ---------------------------------------------------------------------------

#[test]
pub(super) fn a_desk_that_did_not_opt_in_has_no_federation_at_all() {
    let manifest = two_desks(PLAIN);
    let desk = desk_of(&manifest, "eng").expect("a room");
    assert!(
        desk_federation(&record(&manifest), &desk).is_none(),
        "referral is off unless the manifest says otherwise, so the default company \
         deliberates exactly as it did before referral existed"
    );
}

#[test]
pub(super) fn a_company_with_one_desk_has_nowhere_to_refer_to() {
    // The opt-in is present and the fold would honour it; there is simply no
    // peer. `None` here is what keeps the driver from rendering a peer block
    // listing nobody.
    let manifest = format!(
        "[company]\nname = \"Acme\"\n\
         [[agent]]\nid = \"planner\"\nrole = \"Planner\"\n\
         [[agent]]\nid = \"scout\"\nrole = \"Scout\"\n\
         [[group_chat]]\nid = \"eng\"\nname = \"Engineering\"\n\
         members = [\"planner\", \"scout\"]\n{REFERRING}\n"
    );
    let desk = desk_of(&manifest, "eng").expect("a room");
    assert!(desk_federation(&record(&manifest), &desk).is_none());
}

#[test]
pub(super) fn the_federation_carries_every_desk_and_names_the_peers() {
    let manifest = two_desks(REFERRING);
    let desk = desk_of(&manifest, "eng").expect("a room");
    let federation = desk_federation(&record(&manifest), &desk).expect("a federation");

    assert_eq!(
        federation.desks.len(),
        2,
        "both desks are carried — `@#eng` has to resolve to *this* desk for the fold to \
         refuse it as a self-desk mention: {federation:?}"
    );
    let peers = federation.peers_of("eng");
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].id, "platform");
    assert_eq!(peers[0].members, vec!["sre".to_owned(), "dba".to_owned()]);
    assert_eq!(
        federation.agents.len(),
        5,
        "every seated teammate on either desk, so `@sre` resolves from `eng`"
    );
}

/// **A desk created at runtime through the console is a referral peer too.**
///
/// `desk_federation` used to build its desk list from `record.manifest
/// .group_chats` alone, so an `overlay_desks` desk — the console's "create a
/// desk" action — could never be named by `@#id` from another desk, even
/// though [`CompanyRecord::effective_desk_members`] (the single source of
/// truth the REST desk list and the harness both read) already treats
/// manifest and overlay desks as one set.
#[test]
pub(super) fn the_federation_includes_a_console_created_desk() {
    let manifest = two_desks(REFERRING);
    let desk = desk_of(&manifest, "eng").expect("a room");
    let mut record = record(&manifest);
    record.overlay_desks.push(crate::ports::types::OverlayDesk {
        id: "growth".to_owned(),
        name: "Growth".to_owned(),
        description: Some("Runs experiments".to_owned()),
        members: vec!["planner".to_owned()],
        responder: crate::ports::types::ResponderMode::default(),
        hive: Default::default(),
    });

    let federation = desk_federation(&record, &desk).expect("a federation");
    assert_eq!(
        federation.desks.len(),
        3,
        "the manifest's two desks plus the console-created one: {federation:?}"
    );
    let peers = federation.peers_of("eng");
    assert!(
        peers.iter().any(|peer| peer.id == "growth"),
        "a console-created desk must be a reachable referral peer: {peers:?}"
    );
}

/// **A teammate's overlay-edited label reaches the federation, not the
/// manifest's stale one.**
///
/// `member_of` used to read `record.manifest.agents` directly to build the
/// `(id, label)` pairs in `HiveFederation.agents`, bypassing
/// `CompanyRecord::effective_agent` — the function that already applies a
/// console edit on top of the manifest row everywhere else the roster is
/// read. A teammate renamed after the manifest was authored was therefore
/// served under its stale manifest label to a peer desk asking for it.
#[test]
pub(super) fn the_federation_serves_an_overlay_edited_label_not_the_stale_manifest_one() {
    let manifest = two_desks(REFERRING);
    let desk = desk_of(&manifest, "eng").expect("a room");
    let mut record = record(&manifest);
    record
        .overlay_agent_edits
        .push(crate::ports::types::AgentOverride {
            provider: None,
            agent_id: "sre".to_owned(),
            name: Some("Senior SRE".to_owned()),
            role: None,
            description: None,
            tools: None,
            instructions: None,
            avatar: None,
            model: None,
            harness: None,
        });

    let federation = desk_federation(&record, &desk).expect("a federation");
    let label = federation
        .agents
        .iter()
        .find(|(id, _)| id == "sre")
        .map(|(_, label)| label.clone())
        .expect("sre is seated on the peer desk");
    assert_eq!(
        label, "Senior SRE",
        "the overlay-edited label must reach the federation, not the manifest's own: \
         {federation:?}"
    );
}

// ---------------------------------------------------------------------------
// The policy
// ---------------------------------------------------------------------------

#[test]
pub(super) fn an_unopted_desk_derives_a_policy_that_refers_nothing() {
    let config = ReferralConfig::default();
    let policy = config.policy();
    assert!(!policy.enabled);
    assert!(!policy.reach.crosses());
}

#[test]
pub(super) fn opting_in_and_saying_nothing_else_buys_a_round_trip_across_desks() {
    let config = ReferralConfig {
        enabled: Some(true),
        ..ReferralConfig::default()
    };
    let policy = config.policy();
    assert!(policy.enabled);
    // Two hops, because a round trip is one out and one home. A default of one
    // would ask a question and throw the answer away.
    assert_eq!(policy.max_hops, 2);
    assert!(policy.returns);
    assert!(
        policy.reach.addresses_desks(),
        "a desk that opted in and got `local` would have opted in to nothing it \
         could not already do"
    );
    assert_eq!(config.peer_cap(), 2);
}

#[test]
pub(super) fn a_named_reach_is_honoured_in_both_narrower_directions() {
    for (word, crosses, desks) in [
        ("local", false, false),
        ("channels", true, false),
        ("desks", true, true),
    ] {
        let config = ReferralConfig {
            enabled: Some(true),
            reach: Some(word.to_owned()),
            ..ReferralConfig::default()
        };
        let reach = config.policy().reach;
        assert_eq!(reach.crosses(), crosses, "{word}");
        assert_eq!(reach.addresses_desks(), desks, "{word}");
    }
}
