use super::*;
use crate::company::setup::MAX_DESCRIPTION;

#[test]
fn only_the_two_prose_fields_are_draftable() {
    assert_eq!(
        ProfileField::parse("description"),
        Some(ProfileField::Description)
    );
    assert_eq!(
        ProfileField::parse(" instructions "),
        Some(ProfileField::Instructions)
    );
    for other in ["name", "role", "tools", "model", "", "Description"] {
        assert_eq!(ProfileField::parse(other), None, "{other}");
    }
}

/// The bound is the field's own, applied here rather than trusted to the
/// console — a caller that is not our console gets the same clamp.
#[test]
fn a_long_mandate_is_clamped_to_the_card() {
    let long = "x ".repeat(MAX_DESCRIPTION);
    let draft = ProfileDraft::from_answer(ProfileField::Description, "here you go", Some(&long));
    let text = draft.text().expect("a long answer still drafts");
    assert!(
        text.chars().count() <= MAX_DESCRIPTION + 1,
        "clamped to the card: {} chars",
        text.chars().count()
    );
}

/// A persona is bounded by prompt weight, not by the card — the two limits
/// are different in kind, so a persona well over the mandate bound survives.
#[test]
fn a_persona_is_not_clamped_to_the_mandate_bound() {
    let persona = "Confirm the budget before launching. ".repeat(20);
    let draft =
        ProfileDraft::from_answer(ProfileField::Instructions, "tightened it", Some(&persona));
    let text = draft.text().expect("a persona drafts");
    assert!(
        text.chars().count() > MAX_DESCRIPTION,
        "a persona is not held to the card's one line: {} chars",
        text.chars().count()
    );
}

/// Nothing said and nothing drafted is not a turn.
#[test]
fn an_empty_turn_is_unreadable_rather_than_a_blank_suggestion() {
    for blank in ["", "   ", "\n\t "] {
        let draft = ProfileDraft::from_answer(ProfileField::Instructions, blank, Some(blank));
        assert_eq!(draft.refusal(), Some(DraftRefusal::Unreadable), "{blank:?}");
        assert_eq!(draft.text(), None);
        assert_eq!(
            ProfileDraft::from_answer(ProfileField::Instructions, blank, None).refusal(),
            Some(DraftRefusal::Unreadable),
            "{blank:?}"
        );
    }
}

/// A question with no draft is a good turn — it is what lets the copilot
/// find out what the operator means instead of guessing at a paragraph.
#[test]
fn a_question_without_a_draft_is_a_real_turn() {
    let turn = ProfileDraft::from_answer(
        ProfileField::Instructions,
        "Should they be able to sign off releases themselves, or does that go to the lead?",
        None,
    );
    assert_eq!(turn.refusal(), None);
    assert_eq!(turn.text(), None, "a question drafts nothing");
    assert!(
        turn.reply()
            .expect("it said something")
            .contains("sign off")
    );
}

/// A blank draft beside a real reply is a question, not an empty
/// suggestion card.
#[test]
fn a_reply_with_a_blank_draft_drafts_nothing() {
    let turn =
        ProfileDraft::from_answer(ProfileField::Description, "What do they own?", Some("  "));
    assert_eq!(turn.text(), None);
    assert_eq!(turn.refusal(), None);
}

/// The conversation is bounded host-side: oldest turns drop first, each
/// turn is clamped, and blank turns never reach the prompt.
#[test]
fn a_long_conversation_keeps_its_tail() {
    let turns: Vec<CopilotTurn> = (0..MAX_TURNS + 6)
        .map(|i| CopilotTurn {
            role: if i % 2 == 0 {
                TurnRole::Operator
            } else {
                TurnRole::Copilot
            },
            text: format!("turn {i}"),
        })
        .collect();
    let kept = clamp_conversation(turns);
    assert_eq!(kept.len(), MAX_TURNS);
    assert_eq!(
        kept.first().expect("kept").text,
        "turn 6",
        "the oldest drop first"
    );
    assert_eq!(
        kept.last().expect("kept").text,
        format!("turn {}", MAX_TURNS + 5)
    );
}

#[test]
fn a_conversation_drops_blanks_and_clamps_each_turn() {
    let kept = clamp_conversation(vec![
        CopilotTurn {
            role: TurnRole::Operator,
            text: "   ".to_string(),
        },
        CopilotTurn {
            role: TurnRole::Operator,
            text: "x".repeat(MAX_TURN_CHARS + 500),
        },
    ]);
    assert_eq!(kept.len(), 1, "a blank turn is not a turn");
    assert_eq!(kept[0].text.chars().count(), MAX_TURN_CHARS);
}

/// A turn whose speaker cannot be established is dropped rather than
/// guessed at — attributing the operator's words to the copilot is how a
/// conversation starts arguing with itself.
#[test]
fn only_the_two_known_speakers_parse() {
    assert_eq!(TurnRole::parse("operator"), Some(TurnRole::Operator));
    assert_eq!(TurnRole::parse(" copilot "), Some(TurnRole::Copilot));
    for other in ["system", "assistant", "user", ""] {
        assert_eq!(TurnRole::parse(other), None, "{other}");
    }
}

#[test]
fn a_refusal_names_the_operators_next_move() {
    assert_eq!(DraftRefusal::NoModel.as_str(), "no_model");
    assert_eq!(DraftRefusal::ModelUnreachable.as_str(), "model_unreachable");
    assert_eq!(DraftRefusal::Unreadable.as_str(), "unreadable");
}

/// A designed teammate is three fields or none (issue #1989).
///
/// The all-or-nothing rule, and the reason for it: a teammate holding a
/// real mandate and a fragment for a role is what shipped before this, and
/// on screen it looks finished.
#[test]
fn a_design_needs_all_three_fields() {
    assert!(
        TeammateDesign::from_parts(
            "Wholesale Account Manager",
            "Owns stockists.",
            "Be terse.",
            "Runs the stockist channel end to end."
        )
        .is_some()
    );
    for (role, description, instructions) in [
        ("", "Owns stockists.", "Be terse."),
        ("  ", "Owns stockists.", "Be terse."),
        ("Manager", "", "Be terse."),
        ("Manager", "   ", "Be terse."),
        ("Manager", "Owns stockists.", ""),
        ("Manager", "Owns stockists.", "  \n "),
    ] {
        assert!(
            TeammateDesign::from_parts(
                role,
                description,
                instructions,
                "Runs the stockist channel end to end."
            )
            .is_none(),
            "({role:?}, {description:?}, {instructions:?}) must not become a teammate"
        );
    }
}

/// A role too long to be a job title is refused, never cut.
///
/// This is the whole defect, stated as an invariant. The console used to
/// take the operator's sentence, cut it at sixty characters and append `…`,
/// and store the result as a permanent job title — read back on every
/// roster card, interpolated unguarded into `persona_prompt`, and rendered
/// beside the id in the orchestrator's Team block. A truncated role is
/// worse than no role: no role is a question somebody gets asked, and a
/// truncated one is a record nobody was shown.
#[test]
fn a_role_that_would_need_cutting_is_refused() {
    let long = "a".repeat(MAX_ROLE + 1);
    assert!(
        TeammateDesign::from_parts(
            &long,
            "Owns stockists.",
            "Be terse.",
            "Runs the stockist channel end to end."
        )
        .is_none()
    );
    // The bound itself is a bound, not a cut point.
    let exact = "b".repeat(MAX_ROLE);
    let design = TeammateDesign::from_parts(
        &exact,
        "Owns stockists.",
        "Be terse.",
        "Runs the stockist channel end to end.",
    )
    .expect("a role exactly at the bound is fine");
    assert_eq!(design.role, exact);
    assert!(!design.role.contains('…'));
}

/// Whitespace inside a role is collapsed and the edges trimmed, so a model
/// that answered across two lines does not store a job title with a newline
/// in the middle of it — that reaches the persona line verbatim.
#[test]
fn a_designed_role_is_one_line() {
    let design = TeammateDesign::from_parts(
        "  Wholesale\n  Account   Manager  ",
        "Owns.",
        "Be terse.",
        "Runs the stockist channel end to end.",
    )
    .expect("a multi-line answer is still a role");
    assert_eq!(design.role, "Wholesale Account Manager");
}

/// Punctuation and emoji are not job titles. The console's own guard says
/// the same thing, and this is the half that holds when the console is not
/// the caller.
#[test]
fn a_role_with_nothing_readable_in_it_is_refused() {
    for role in ["🎉🎉", "...", "!?!", "— —"] {
        assert!(
            TeammateDesign::from_parts(
                role,
                "Owns stockists.",
                "Be terse.",
                "Runs the stockist channel end to end."
            )
            .is_none(),
            "{role:?} is not a job title"
        );
    }
}

/// A role the *model* truncated is refused, exactly like one that was too
/// long to fit.
///
/// The length check above cannot catch this: `"Growth Marketer…"` is
/// sixteen characters and every one of them passes. But it is the same
/// stored record the whole change exists to stop — a job title with its end
/// sliced off, read into every prompt this teammate ever runs — and the
/// brief telling the model never to write one is a brief, not a validator.
///
/// Refusing here rather than only in the console is what makes the answer
/// legible. `designedTeammateFields` drops such a design too, but a design
/// that arrived whole carries no `reason`, so the console's hand-over says
/// `"unknown"` — it can tell the operator the dialog changed and not why.
/// A refusal from this side arrives as `Unreadable` and says it.
#[test]
fn a_role_the_model_truncated_is_refused() {
    for role in [
        "Growth Marketer…",
        "Runs wholesale outreach to boutique retailers and keeps the…",
        "Growth Marketer...",
        "Wholesale … Manager",
    ] {
        assert!(
            TeammateDesign::from_parts(
                role,
                "Owns stockists.",
                "Be terse.",
                "Runs the stockist channel end to end."
            )
            .is_none(),
            "{role:?} is a cut-off job title, not a job title"
        );
    }
    // The mandate's own clamp mark is not a truncated role, and a design
    // whose description ends in `…` is still a design.
    let design = TeammateDesign::from_parts(
        "Growth Marketer",
        "Owns stockists…",
        "Be terse.",
        "Runs the stockist channel end to end.",
    )
    .expect("an ellipsis in the mandate is the clamp's own mark");
    assert_eq!(design.role, "Growth Marketer");
}

/// Three non-empty fields that are the same sentence are not a design.
///
/// This is the operator's original complaint, restated as an invariant: one
/// sentence appearing as role, mandate and persona at once. Every length
/// and emptiness check passes, the JSON is valid, and the console would
/// store it — so nothing but this refuses it.
#[test]
fn a_design_that_repeats_itself_is_refused() {
    let sentence = "Runs wholesale outreach to boutique retailers.";
    assert!(
        TeammateDesign::from_parts(
            sentence,
            sentence,
            sentence,
            "Runs the stockist channel end to end."
        )
        .is_none()
    );
    // Any pair of the three, not only all three.
    assert!(
        TeammateDesign::from_parts(
            "Growth Marketer",
            sentence,
            sentence,
            "Runs the stockist channel end to end."
        )
        .is_none(),
        "a persona that restates the mandate is the wrong field, not a design"
    );
    assert!(
        TeammateDesign::from_parts(
            "Growth Marketer",
            "Growth Marketer",
            "Be terse.",
            "Runs the stockist channel end to end."
        )
        .is_none(),
        "a mandate that is only the job title says nothing the role did not"
    );
    assert!(
        TeammateDesign::from_parts(
            "Growth Marketer",
            "Owns stockists.",
            "Growth Marketer",
            "Runs the stockist channel end to end."
        )
        .is_none(),
        "a persona that is only the job title is the same defect from the other end"
    );
    // Normalized, so a full stop or a capital is not a way past it.
    assert!(
        TeammateDesign::from_parts(
            "Manager",
            "Owns stockists",
            "owns stockists.",
            "Runs the stockist channel end to end."
        )
        .is_none(),
        "the same sentence with a keystroke of difference is still the same sentence"
    );
    assert!(
        TeammateDesign::from_parts(
            "Manager",
            "Owns  stockists.",
            "Owns\nstockists.",
            "Runs the stockist channel end to end."
        )
        .is_none(),
        "whitespace is not a distinction between two fields"
    );
    // Three genuinely different fields still design.
    assert!(
        TeammateDesign::from_parts(
            "Wholesale Account Manager",
            "Owns the stockist relationships and the reorder cadence.",
            "Check stock before promising a date. Escalate a missed reorder.",
            "Runs the stockist channel end to end."
        )
        .is_some()
    );
}

/// A role that is a sentence is refused, even when it fits the char bound.
///
/// `MAX_ROLE` bounds characters and a sentence fits inside it — the
/// reported case, `"Handles payroll and reconciles the books weekly"`, is
/// 46 of the 60 allowed, has alphanumerics, no ellipsis, and matches no
/// other field. Every rule but this one passes it, and it would then be
/// stored and read as the teammate's identity in every prompt it runs.
#[test]
fn a_role_that_is_a_sentence_is_refused() {
    let sentence = "Handles payroll and reconciles the books weekly";
    assert!(
        sentence.chars().count() < MAX_ROLE,
        "the char bound does not catch it"
    );
    assert!(
        TeammateDesign::from_parts(
            sentence,
            "Owns payroll accuracy and the monthly close.",
            "Run the cycle on the 25th. Escalate a mismatch before paying.",
            "Sorts out the money side of things.",
        )
        .is_none(),
        "a role of {} words is a sentence, not a job title",
        sentence.split_whitespace().count()
    );

    // Real titles, including ones that run past what the brief asks for,
    // still design. The slack is the whole reason the bound is five.
    for role in [
        "Manager",
        "Growth Marketer",
        "Wholesale Account Manager",
        "Senior Wholesale Account Manager",
        "VP of Brand and Communications",
    ] {
        assert!(
            TeammateDesign::from_parts(
                role,
                "Owns the stockist pipeline and the terms behind it.",
                "Check terms against the price list before quoting.",
                "Sorts out the money side of things.",
            )
            .is_some(),
            "{role:?} is a job title and must design"
        );
    }
}

/// The operator's own sentence handed back as the job title is refused.
///
/// The shape the three-fields-against-each-other rule cannot see, and the
/// one that matters most: a brief of `"Handles payroll"` answered with role
/// `"Handles payroll"`, a real mandate and real instructions passes every
/// other check in `from_parts`. It is the original defect exactly — the
/// operator's sentence stored as a permanent role, interpolated into every
/// prompt that teammate ever runs — and only a comparison against the input
/// catches it.
#[test]
fn a_role_that_is_the_front_of_the_brief_is_refused() {
    // The shape whole-brief equality misses, and the one both reviewers
    // found independently: a clause split, arriving without the ellipsis
    // that used to make it obvious. Every other rule passes it — it is 23
    // characters, three words, no ellipsis, distinct from the mandate and
    // the persona, and not equal to the brief.
    let brief = "Runs wholesale outreach to boutique retailers";
    assert!(
        TeammateDesign::from_parts(
            "Runs wholesale outreach",
            "Owns the stockist pipeline and the terms behind it.",
            "Check terms against the price list before quoting.",
            brief,
        )
        .is_none(),
        "the front of the operator's sentence is a cut, not a job title"
    );
    // Normalized, so case and punctuation are not a way past it.
    assert!(
        TeammateDesign::from_parts(
            "runs wholesale outreach.",
            "Owns the stockist pipeline.",
            "Check terms before quoting.",
            "Runs Wholesale Outreach to boutique retailers",
        )
        .is_none()
    );
    // The word boundary matters: a role must not match a longer first word.
    assert!(
        TeammateDesign::from_parts(
            "Ops",
            "Owns the Opsware migration and its cutover plan.",
            "Stage the cutover behind a flag. Escalate a failed migration.",
            "Opsware migration and cutover",
        )
        .is_some(),
        "\"Ops\" is not the front of \"Opsware migration\""
    );
    // A title the model wrote, against the same brief, still designs — the
    // rule catches fragments of the input, not verbs in general.
    assert!(
        TeammateDesign::from_parts(
            "Wholesale Account Manager",
            "Owns the stockist pipeline and the terms behind it.",
            "Check terms against the price list before quoting.",
            brief,
        )
        .is_some()
    );
}

#[test]
fn a_role_that_is_only_the_brief_is_refused() {
    let brief = "Handles payroll";
    assert!(
        TeammateDesign::from_parts(
            "Handles payroll",
            "Owns payroll accuracy and the monthly deadlines.",
            "Run the payroll cycle on the 25th. Escalate a mismatch before paying.",
            brief,
        )
        .is_none(),
        "the brief handed back as the role is the defect, whatever sits beside it"
    );
    // Normalized, so punctuation and case are not a way past it.
    assert!(
        TeammateDesign::from_parts(
            "handles payroll.",
            "Owns payroll accuracy and the monthly deadlines.",
            "Run the payroll cycle on the 25th.",
            "Handles Payroll",
        )
        .is_none()
    );
    // And whitespace is not a distinction either.
    assert!(
        TeammateDesign::from_parts(
            "Handles   payroll",
            "Owns payroll accuracy.",
            "Run the cycle on the 25th.",
            "Handles\npayroll",
        )
        .is_none()
    );

    // A role the model actually wrote still designs, from the same brief.
    let design = TeammateDesign::from_parts(
        "Payroll Administrator",
        "Owns payroll accuracy and the monthly deadlines.",
        "Run the payroll cycle on the 25th. Escalate a mismatch before paying.",
        brief,
    )
    .expect("a designed role beside the same brief is the good case");
    assert_eq!(design.role, "Payroll Administrator");

    // No brief to compare against is not a refusal — the rule needs an
    // input, and a caller without one still gets every other check.
    assert!(
        TeammateDesign::from_parts("Handles payroll", "Owns payroll.", "Be terse.", "").is_some(),
        "an empty brief cannot make a role a duplicate of anything"
    );
}

/// The mandate and the persona are bounded by the same clamps the fields
/// themselves obey, so a design cannot store what an edit could not.
///
/// A long *mandate* is clamped rather than refused, unlike a long role, and
/// the asymmetry is the point. `clamp_description` cuts on a word boundary
/// and marks the cut with `…` — the same treatment the roster designer's
/// mandates get — because a mandate is a one-line card summary and a
/// shortened one still says what the teammate owns. A role is an identity
/// interpolated into a sentence in every prompt that teammate ever runs,
/// and half of one is not a shorter job title, it is a broken one.
#[test]
fn a_design_is_bounded_by_the_fields_it_fills() {
    let design = TeammateDesign::from_parts(
        "Manager",
        &"m ".repeat(MAX_DESCRIPTION),
        &"p".repeat(200),
        "Runs the stockist channel end to end.",
    )
    .expect("a long answer is still a design");
    // The clamp's own ellipsis is the one character over the layout bound.
    assert!(design.description.chars().count() <= MAX_DESCRIPTION + 1);
    assert!(design.description.ends_with('…'));
    assert!(!design.instructions.is_empty());
}

/// A brief long enough to be cut keeps everything a card would have lost.
///
/// The concrete regression: an operator writing two paragraphs into the one
/// box had everything past character 200 dropped before the model read it,
/// with `clamp_description`'s `…` stitched on the end. Nothing recorded it —
/// the stored description is the model's, not theirs.
#[test]
fn a_design_brief_keeps_what_the_card_bound_would_have_cut() {
    let brief = format!(
        "Runs wholesale outreach to boutique retailers. {}. And they must never quote a \
         price below the trade list without asking Finance first.",
        "x".repeat(MAX_DESCRIPTION)
    );
    assert!(brief.chars().count() > MAX_DESCRIPTION);
    let kept = clamp_design_brief(&brief);
    assert_eq!(kept, brief, "nothing inside the bound is cut");
    assert!(
        kept.ends_with("asking Finance first."),
        "the requirement written past the card bound has to survive: {kept}"
    );
    assert!(
        !kept.ends_with('…'),
        "and no ellipsis is added — a brief ending in one reads to the model \
         as an unfinished sentence"
    );
}

/// Past the prompt bound it is still cut, so a paste cannot push the
/// grounding out of the design prompt.
#[test]
fn a_design_brief_is_still_bounded_by_prompt_weight() {
    let huge = "word ".repeat(MAX_DESIGN_BRIEF);
    assert_eq!(clamp_design_brief(&huge).chars().count(), MAX_DESIGN_BRIEF);
    assert_eq!(clamp_design_brief("  two   spaces  "), "two spaces");
}

/// `clamp_role` is the belt to `from_parts`' braces: the type cannot hold an
/// unbounded string even if a future caller forgets the refusal rule.
#[test]
fn clamp_role_bounds_and_collapses() {
    assert_eq!(clamp_role("  Growth   Marketer "), "Growth Marketer");
    assert_eq!(clamp_role(&"z".repeat(500)).chars().count(), MAX_ROLE);
}
