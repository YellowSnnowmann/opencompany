//! Assembling one agent's system prompt from its manifest definition.
//!
//! An agent's prompt is built from four kinds of material, and the order they
//! appear in is a decision rather than an accident:
//!
//! 1. the generated **persona** — who this teammate is, at which company;
//! 2. its **inline prompt**, the operator's working instruction for the role;
//! 3. its **bundle documents** ([`prompt_files`](crate::company::Agent::prompt_files)),
//!    version controlled beside the agent definition;
//! 4. its **routed documents** ([`context`](crate::company::Agent::context)),
//!    live operator-owned workspace notes.
//!
//! Static material comes first and volatile material last, because the prompt
//! prefix is what a provider cache can reuse across turns: a workspace note the
//! operator edits between two turns invalidates everything after it, so it is
//! placed where "everything after it" is nothing.
//!
//! This module is deliberately **always compiled** and free of any runtime
//! dependency. The harness that consumes it (`crate::harness::build`) is behind
//! the `openhuman` feature, but the composition and clamping rules are ordinary
//! text manipulation with real edge cases — a budget cut that splits a
//! codepoint, a document that is only whitespace — and they are worth testing
//! in the default build rather than only where the agent runtime links.

use crate::company::{Agent, PROMPT_FILE_BUDGET_CHARS};

/// The marker appended to a section that the budget cut short.
///
/// Visible rather than silent, and it names the budget: an agent whose briefing
/// was truncated should be able to say so, and an operator reading the prompt
/// should not have to guess whether the document simply ended there. A silent
/// cut is indistinguishable from a document that was written short.
pub const TRUNCATION_MARKER: &str = "\n\n[… truncated to fit the prompt budget]";

/// The heading introducing the routed-document section.
const CONTEXT_HEADING: &str = "\n\n## Working documents\n\nYou are told to reason from the documents below. They are the company's current \
working state, not background reading.\n";

/// The heading introducing the bundle-document section.
const BUNDLE_HEADING: &str = "\n\n## Your brief\n";

/// The persona sentence for a company agent, plus the operator's inline prompt.
///
/// Frames the agent as its manifest role at the company, in the first person.
/// This is what makes the agent answer *as* the CEO of Acme rather than falling
/// back to the runtime's own assistant identity.
///
/// An agent carrying a [`name`](Agent::name) — an operator-added teammate — is
/// framed as that name *and* the role, because the console addresses it by name
/// everywhere (DM header, subtitle, composer) and an agent told only its role
/// contradicts the interface it is speaking through (issue #1105). The name is
/// stated as an address, not a character: a teammate should answer to it
/// without inventing a persona around it.
///
/// The `instructions` — the agent's **effective** persona text, resolved by the
/// caller through [`CompanyRecord::effective_instructions`](crate::ports::types::CompanyRecord::effective_instructions)
/// (an operator override when one is set, else the manifest agent's `prompt`,
/// else `None`) — are **appended** to that framing rather than replacing it: an
/// operator writing instructions is stating how the role should work, not
/// disclaiming which role it is, and text that replaced the framing would
/// silently cost the agent its identity (issue #1530).
///
/// Taken as a parameter rather than read off `agent.prompt` so the single
/// injection point serves both agent kinds uniformly: a manifest agent whose
/// persona an operator edited from the console, and an overlay teammate that has
/// no manifest `prompt` at all, both arrive here as the same resolved
/// `Option<&str>`. A blank or whitespace-only value adds nothing.
pub fn persona_prompt(company_name: &str, agent: &Agent, instructions: Option<&str>) -> String {
    // Blank is absent, as it is for `description` and `prompt` below. A name
    // that just restates the role is dropped too, or the framing reads "You are
    // Content Writer, the Content Writer at Acme."
    let named = agent
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case(agent.role.trim()));
    let mut prompt = match named {
        Some(name) => format!(
            "You are {name}, the {role} at {company}. Speak in the first person as this role. \
             Teammates and the operator address you as {name}; it is how you are called here, \
             not a separate character to play.",
            name = name,
            role = agent.role,
            company = company_name,
        ),
        None => format!(
            "You are the {role} at {company}. Speak in the first person as this role.",
            role = agent.role,
            company = company_name,
        ),
    };
    if let Some(description) = agent.description.as_deref() {
        let description = description.trim();
        if !description.is_empty() {
            prompt.push(' ');
            prompt.push_str(description);
        }
    }
    if let Some(custom) = instructions {
        let custom = custom.trim();
        if !custom.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(custom);
        }
    }
    prompt
}

/// The bundle-document section for an agent, or `""` when it has none.
///
/// Reads the bodies the manifest loader already resolved
/// ([`prompt_files_resolved`](crate::company::Agent::prompt_files_resolved)), so
/// this does no I/O and can run on every roster rebuild.
pub fn bundle_section(agent: &Agent) -> String {
    let documents: Vec<(&str, &str)> = agent
        .prompt_files_resolved
        .iter()
        .map(|(path, body)| (path.as_str(), body.as_str()))
        .collect();
    document_section(BUNDLE_HEADING, &documents)
}

/// The routed-document section for `documents`, or `""` when there are none.
///
/// Takes the resolved `(name, body)` pairs rather than reading them, because the
/// workspace store is async and the agent build is not — the caller resolves
/// them ahead of time, the same way it resolves skill deltas.
pub fn context_section(documents: &[(String, String)]) -> String {
    let documents: Vec<(&str, &str)> = documents
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_str()))
        .collect();
    document_section(CONTEXT_HEADING, &documents)
}

/// Renders a titled list of documents under `heading`, clamped as a whole.
///
/// The budget applies to the **section**, not to each document: a role routed
/// five documents and a role routed one are spending from the same prompt, and a
/// per-document budget would let the first quietly cost five times the second.
///
/// Empty and whitespace-only bodies are dropped rather than rendered as a bare
/// heading with nothing under it — an empty section reads to the model as a
/// document that exists and says nothing, which is worse than its absence.
fn document_section(heading: &str, documents: &[(&str, &str)]) -> String {
    let mut body = String::new();
    for (name, content) in documents {
        if content.trim().is_empty() {
            continue;
        }
        body.push_str("\n### ");
        body.push_str(name);
        body.push('\n');
        body.push_str(content.trim_end());
        body.push('\n');
    }
    if body.is_empty() {
        return String::new();
    }
    format!("{heading}{}", clamp(&body, PROMPT_FILE_BUDGET_CHARS))
}

/// Clamps `text` to `budget` codepoints, keeping the leading portion.
///
/// Three properties, each of which prevents a specific failure:
///
/// * It cuts on a **character** boundary, never a byte one, so a multi-byte
///   codepoint at the limit is dropped whole rather than sliced into invalid
///   UTF-8.
/// * It keeps the **leading** portion, because these documents are written
///   most-important-first — a brief leads with what is established, an operator
///   brief leads with the instruction.
/// * It marks the cut, so truncation is legible instead of looking like a short
///   document.
///
/// Clamping happens here, where the text is spent, rather than at load: refusing
/// or truncating the read would cost the company the whole document, while
/// clamping at assembly costs only its tail.
pub fn clamp(text: &str, budget: usize) -> String {
    // `chars().count()` walks the string, so only pay for it when the cheap byte
    // length says a cut is even possible (bytes >= chars, always).
    if text.len() <= budget {
        return text.to_string();
    }
    let mut kept: String = text.chars().take(budget).collect();
    if kept.chars().count() == text.chars().count() {
        return kept;
    }
    kept.push_str(TRUNCATION_MARKER);
    kept
}

/// Caps operator-authored persona instructions to the prompt budget.
///
/// `instructions` written through the team/agent edit surfaces are injected,
/// verbatim, into every turn of the teammate's system prompt via
/// [`persona_prompt`]. Unlike `bundle_section`/`context_section`, that injection
/// point applied no budget of its own — the persona grew without ceiling as an
/// operator pasted more text, inflating every dispatch. Capping here, at the
/// write boundary (mirroring [`crate::ports::tasks::cap_discussion`]), keeps the
/// stored override bounded without refusing an operator's edit; the leading,
/// most-important portion is preserved and a cut is marked.
pub fn cap_persona_instructions(text: &str) -> String {
    clamp(text, PROMPT_FILE_BUDGET_CHARS)
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
