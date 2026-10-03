// Every word the desktop analytics disclosure and the Privacy page say, in one
// place.
//
// # Why one file
//
// This is privacy copy: what is collected, and what is promised never to be.
// It must be reviewable by someone who will not read a React component, and it
// must not drift between the first-launch notice and the Settings page that
// repeats it. **LEGAL / OPERATOR APPROVAL OF THIS WORDING IS PENDING** (see the
// PR description) — edit it here and nowhere else.
//
// # Every claim is checked against the code
//
// Each line of `ANALYTICS_COLLECTED` and `ANALYTICS_NEVER_COLLECTED` is a
// restatement of the payload contract, not of intent:
//
//   - `crates/opencompany-core/src/analytics/types.rs` — a property value is a
//     word from a compiled-in list, a count, an amount or a flag; there is no
//     string variant, so free text cannot be sent.
//   - `types/event.rs` — the three events and their properties
//     (`instance_started.companies`, `turn_finished.{trigger,outcome,failure,
//     duration_ms,effects_executed,approvals_parked}`,
//     `turn_metered.{provider,model,input_tokens,output_tokens,cost_usd,…}`).
//   - `docs/spec/runtime/analytics.md` — the context envelope (`app_version`,
//     `shell_version`, `os`, `arch`, build features) and the never-collected
//     list.
//   - `analytics_pii_tests.rs` — the exhaustive no-PII property.
//
// If the payload gains a field, this list must gain a line in the same change.

/** The page this wording is the plain-language form of. */
export const ANALYTICS_DOCS_URL =
  "https://github.com/tinyhumansai/opencompany/blob/main/docs/spec/runtime/analytics-desktop.md";

/** What is sent, one plain sentence each. */
export const ANALYTICS_COLLECTED: readonly string[] = [
  "The app version, your operating system and your CPU architecture.",
  "A random install ID. It identifies this installation of the app, not you, and it is not derived from your name, email, account or computer name.",
  "Counts and sizes, such as how many companies this app runs.",
  "How each turn of work ended (finished or failed, and a broad reason), how long it took, and how many actions and approvals it involved.",
  "Token counts and cost for each model call, with the model provider and model family from a fixed list.",
  "Which optional features this build of the app includes.",
];

/** What is never sent. */
export const ANALYTICS_NEVER_COLLECTED: readonly string[] = [
  "Message content, prompts or anything an agent writes.",
  "File names or file paths.",
  "Names of companies, agents, tasks or tools.",
  "Email addresses or any other personal details.",
  "Passwords, API keys or any other credentials.",
];

/** The network caveat that is true of any request and so is said plainly. */
export const ANALYTICS_IP_NOTE =
  "As with any network request, the collector can see your IP address when an event arrives.";

/** The first-launch notice. */
export const DISCLOSURE = {
  title: "Help improve OpenCompany",
  body: "This app shares anonymous usage data: your app version, operating system, counts such as how many companies you run, and how turns of work end and what they cost. Never your messages, files, names, emails or credentials.",
  where: "You can change this any time in Settings → Privacy.",
  turnOff: "Turn off",
  gotIt: "Got it",
  failed: "Could not save that choice. It will be asked again next time.",
} as const;

/** The Privacy settings page. */
export const PRIVACY = {
  title: "Privacy",
  toggleTitle: "Share anonymous usage data",
  toggleDescription:
    "Helps us see which parts of OpenCompany work and which do not. Turning it off stops sending right away.",
  toggleLabel: "Share anonymous usage data",
  restartNote:
    "Turning this on takes effect the next time you open the app. Nothing is sent until then.",
  envNote:
    "This is set by the OPENCOMPANY_ANALYTICS environment variable, which overrides the switch. Change or remove the variable to change it here.",
  collectedTitle: "What is sent",
  collectedIntro: "Only these things, and only as counts, short fixed words and numbers:",
  neverTitle: "What is never sent",
  statusTitle: "Last send",
  statusOff: "Not sending. Nothing is queued or sent while this is off.",
  statusUnavailable: "No status is available yet. It appears once the app has started a company.",
  docsLink: "Read the full technical description",
  saveFailed: "Could not save your choice",
  loadFailed: "Could not read the current setting",
} as const;

/** A person's reading of each last-send outcome. Keys are `AnalyticsLastSend`. */
export const LAST_SEND_LABEL: Record<string, string> = {
  never: "Nothing sent yet",
  accepted: "Accepted by the collector",
  "refused-credential": "Refused by the collector",
  redirect: "The collector redirected the request",
  "collector-busy": "The collector is busy",
  unreachable: "The collector could not be reached",
  "rejected-event": "The collector rejected an event",
};
