//! Reading a failed provider check: what class of failure it is, what to say
//! about it, and where a probe is allowed to point.
//!
//! Everything here is **pure**. The network call itself lives at the edge, in
//! the route that performs it; what this module holds is the part with branches
//! worth testing — and it is testable with a string and no host.
//!
//! ## Why classification is separate from wording
//!
//! [`classify`] decides, [`describe`] says. Splitting them is what makes the six
//! classes testable without a copy deck, and it keeps strings where strings
//! belong. It is also how the design being ported does it.
//!
//! ## Why only one class deletes the key
//!
//! Adding a provider writes the credential, then probes. If the probe fails, the
//! naive answer is "roll everything back", and the naive answer **destroys valid
//! credentials**: a corporate proxy, a WAF, a rate limit or a mistyped model id
//! all fail a probe while the key is perfectly good.
//!
//! So only [`ProbeClass::Auth`] is destructive. Everything else keeps the key and
//! the record and shows an amber advisory, because the key is plausibly fine and
//! the *connection* is not. Colouring those as errors would be a lie about what
//! happened — the save succeeded.
//!
//! ## The branch order is the whole design
//!
//! Two orderings exist because of real failures, and both are easy to
//! "simplify" back into the bug:
//!
//! **Proxy and gateway rejections are checked FIRST.** The phrase `407 Proxy
//! Authentication Required` contains the word *authentication*. Check the auth
//! branch first and a corporate proxy deletes a valid key. A WAF's bare `403
//! Forbidden` has the same shape, which is why the status-code tests use word
//! boundaries — so `401` and `403` do not match inside an id like `1403`.
//!
//! ## Only the vendor's words decide, never ours
//!
//! [`classify`] reads a string this module builds, so any text this module adds
//! to it is text the rules can match against themselves. That is not theoretical:
//! the failure string used to carry the status' own reason phrase, and the auth
//! branch tested for `forbidden` — which `canonical_reason()` supplies on every
//! single 403. The guard read as "a 403 counts only with credential wording" and
//! behaved as "every 403 deletes the key", across every provider in the
//! catalogue, for causes as ordinary as a prompt that ran past the model's
//! context window.
//!
//! So [`build_failure_text`] passes the status code and the vendor's body and
//! nothing else, and the auth rule is a **positive** list of published
//! credential-refusal phrases ([`says_the_credential_was_refused`]) rather than a
//! denylist of four words. A body nobody anticipated is now `Unknown`, which
//! keeps the key.
//!
//! **`model` is checked BEFORE `endpoint`.** The endpoint branch matches a bare
//! "not found", which would otherwise claim every provider that phrases a
//! missing model as "model not found" and send the operator off to check their
//! base URL instead of their model id.
//!
//! ## And the raw string never reaches the copy
//!
//! [`describe`] does not interpolate the upstream text. That text can echo
//! request material — headers, key fragments — and it lands in a banner someone
//! screenshots. The raw string belongs in a detail channel, not in the sentence.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Duration;

use super::catalogue;

/// What a failed probe means.
///
/// Six named classes rather than a boolean, because each one has a different
/// remedy and — more importantly — a different answer to "should the credential
/// we just wrote be deleted?".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeClass {
    /// The provider rejected the credential. **The only destructive class.**
    Auth,
    /// The endpoint answered but does not know that model id.
    Model,
    /// The account is out of credit, or rate limited.
    Quota,
    /// Nothing answered at that address.
    Endpoint,
    /// Something answered too slowly.
    Timeout,
    /// The check did not complete, and we will not guess why.
    Unknown,
}

impl ProbeClass {
    /// Whether meeting this class should roll back the credential that was just
    /// written.
    ///
    /// Exactly one class says yes. If a second ever does, re-read the module
    /// header first — every other class is a connection fact, not a key fact.
    pub fn destroys_credential(self) -> bool {
        matches!(self, Self::Auth)
    }

    /// The stable wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::Model => "model",
            Self::Quota => "quota",
            Self::Endpoint => "endpoint",
            Self::Timeout => "timeout",
            Self::Unknown => "unknown",
        }
    }
}

/// Whether `needle` appears in `haystack` delimited by non-word characters —
/// the `\b…\b` a regex would give, without pulling in a regex.
///
/// This is what stops `403` matching inside `1403` or `4032`. It is not a
/// nicety: a model id or a request id with those digits in it would otherwise
/// be read as a status code and delete the operator's key.
fn contains_token(haystack: &str, needle: &str) -> bool {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let bytes = haystack.as_bytes();
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(needle) {
        let start = from + offset;
        let end = start + needle.len();
        let before_ok = start == 0 || !is_word(bytes[start - 1] as char);
        let after_ok = end == bytes.len() || !is_word(bytes[end] as char);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
        if from >= haystack.len() {
            break;
        }
    }
    false
}

/// Whether the body says the **credential itself** was refused, as opposed to
/// saying the credential is fine and something else about the request is not.
///
/// A positive list on purpose. The rule it replaces was a denylist — four words
/// that, if absent, let a 403 delete a key — and a denylist of failure wordings
/// cannot be complete, because it has to anticipate every phrase 29 vendors
/// might use for a cause nobody has thought of yet. Inverting it makes the
/// unanticipated case non-destructive: a body we do not recognise keeps the key.
///
/// The phrases are the ones vendors actually publish. `authentication` appears
/// here only in compound forms, never as the bare word: it used to match on its
/// own, so an endpoint answering *"Bearer authentication is not supported, use
/// x-api-key"* — a 400 about our request shape, with a perfectly good key —
/// classified as a rejected credential and deleted it. Groq's `424` for a failed
/// downstream *"(e.g., Remote MCP authentication)"* is the same shape.
fn says_the_credential_was_refused(haystack: &str) -> bool {
    const REFUSALS: &[&str] = &[
        // OpenAI, Groq and everything that copied their wording.
        "invalid api key",
        "invalid_api_key",
        "incorrect api key",
        // Fireworks publishes exactly these two and neither matches the three
        // above, so without them its genuine bad-key 403 and 401 both read as
        // "we could not tell" and a dead key is kept forever.
        "api key you provided is invalid",
        "must provide an api key",
        // Google's compat surface, whose word order matches none of the above.
        "api key not valid",
        // Anthropic's and DeepSeek's typed bodies.
        "authentication_error",
        "authentication failed",
        "authentication fails",
        "invalid authentication",
        "invalid credential",
        "invalid_credential",
        "bad credentials",
        "missing api key",
        "no api key provided",
        // Venice's typed code, and the bare word as a body signal. Now that the
        // reason phrase is not synthesised in, a haystack containing this word
        // means the vendor wrote it.
        "authentication_failed",
        "unauthorized",
    ];
    REFUSALS.iter().any(|phrase| haystack.contains(phrase))
}

/// Which failure a raw provider error string represents.
///
/// Ported branch for branch — including the order — from the design this work
/// follows. **Reordering these branches is a behaviour change**, not a
/// refactor; the module header names the two that matter and what each prevents.
pub fn classify(raw: &str) -> ProbeClass {
    let haystack = raw.trim().to_ascii_lowercase();

    // Network, gateway and proxy rejections are about the CONNECTION, not the
    // key. They must not reach the auth branch, or the add flow deletes a valid
    // key over a corporate proxy, a WAF, or a 407 challenge — the exact class
    // this ordering exists to preserve keys through. Checked first so
    // "authentication" inside "407 Proxy Authentication Required", and a
    // WAF/Cloudflare "403 Forbidden", classify as `unknown`.
    if contains_token(&haystack, "407")
        || haystack.contains("proxy")
        || haystack.contains("cloudflare")
        || haystack.contains("bad gateway")
        || haystack.contains("gateway timeout")
    {
        return ProbeClass::Unknown;
    }

    // A rejected credential — and **only** a rejected credential, because this
    // is the one class that deletes the operator's key.
    //
    // `401` is the single status that is, on its own, a statement about the
    // credential. Every other status reaches this class through the body and
    // nothing else, including `403`.
    //
    // **There is deliberately no 403 rule here.** There used to be one: a 403
    // counted as auth when it co-occurred with `forbidden`/`key`/`credential`/
    // `permission`. It matched every 403 ever seen, because the string this
    // classifier reads was built with the status' own reason phrase in it —
    // literally `Forbidden` — so the guard tested our own text rather than the
    // vendor's. `build_failure_text` no longer synthesises it, and the rule that
    // depended on it is gone rather than repaired, because every disjunct was
    // wrong on its own terms:
    //
    // * `forbidden` is the reason phrase, which vendors also echo in the body;
    // * `permission` is how Anthropic (`permission_error`), Google
    //   (`PERMISSION_DENIED`), Groq, xAI and Cerebras all phrase an
    //   **entitlement** failure by a key that is perfectly valid;
    // * `key` matches Anthropic's *"Your API key does not have permission to use
    //   the specified resource"* — a working key, named in its own refusal.
    //
    // The documented 403s across the catalogue are overwhelmingly not about the
    // credential: Together returns one for a context-length overflow, OpenAI for
    // geography, Fireworks for data residency, xAI for a blocked team, OpenRouter
    // for a moderation flag. Fireworks is the one provider that genuinely 403s a
    // bad credential, and it says so in words — *"The API key you provided is
    // invalid"*, *"You must provide an API key"* — so it reaches this class
    // through the body list below, like every other vendor.
    //
    // A 403 whose body says nothing recognisable now falls through to `Unknown`,
    // which keeps the key. That is the safe direction: a kept key that does not
    // work is a second attempt, and a deleted key that did work is unrecoverable.
    if contains_token(&haystack, "401") || says_the_credential_was_refused(&haystack) {
        return ProbeClass::Auth;
    }

    // Before `endpoint`, on purpose: the endpoint branch matches a bare "not
    // found", which would otherwise claim every provider that phrases a missing
    // model as "model not found" and send the operator off to check their base
    // URL instead of their model id.
    if haystack.contains("model_not_found")
        || (haystack.contains("not found") && haystack.contains("model"))
        || haystack.contains("does not exist")
        || haystack.contains("is not available")
        || haystack.contains("unknown model")
        || haystack.contains("invalid model")
    {
        return ProbeClass::Model;
    }

    if haystack.contains("quota")
        || haystack.contains("insufficient")
        || haystack.contains("billing")
        || haystack.contains("429")
        || haystack.contains("rate limit")
    {
        return ProbeClass::Quota;
    }

    // "404 / not found / DNS / refused" — all four, not the first two. A
    // connection that was refused and a name that did not resolve are the
    // clearest possible evidence that nothing is at that address, and reading
    // them as `unknown` sent the operator to look at their key instead of their
    // URL for the most common typo there is.
    if haystack.contains("404")
        || haystack.contains("not found")
        || haystack.contains("refused")
        || haystack.contains("unreachable")
        || haystack.contains("dns")
        || haystack.contains("no such host")
        || haystack.contains("could not resolve")
        || haystack.contains("name resolution")
        || haystack.contains("connection reset")
    {
        return ProbeClass::Endpoint;
    }

    if haystack.contains("timeout") || haystack.contains("timed out") {
        return ProbeClass::Timeout;
    }

    ProbeClass::Unknown
}

/// Whether meeting this class should undo the add, given what kind of provider
/// it was.
///
/// [`ProbeClass::destroys_credential`] answers the general rule: only a rejected
/// credential is evidence about the credential, so only that class rolls one
/// back. This adds the one category-specific exception, and it is in the design
/// this ports:
///
/// **A local runtime rolls back on an unreachable endpoint too.** A runtime that
/// is not running is not a connection worth creating — the operator's next move
/// is to start it and retry, not to keep a row that points at a port with
/// nothing behind it. For a cloud provider the same class means the opposite: a
/// proxy, a WAF or a slow gateway sits between a perfectly good key and an
/// endpoint that is fine, which is why that case keeps both.
///
/// The asymmetry is the point. `endpoint` against `127.0.0.1:11434` is a fact
/// about the operator's machine; `endpoint` against `api.acme.dev` is a fact
/// about the network in between.
pub fn rolls_back(class: ProbeClass, category: catalogue::Category) -> bool {
    if class.destroys_credential() {
        return true;
    }
    matches!(category, catalogue::Category::Local)
        && matches!(class, ProbeClass::Endpoint | ProbeClass::Timeout)
}

/// What to tell the operator, given a class and the provider's label.
///
/// **Never interpolates the raw upstream string.** That text can carry request
/// material — headers, fragments of a key — and this sentence lands in a banner
/// that gets screenshotted and pasted into a ticket. The raw text goes to a
/// detail or console channel instead.
///
/// Every sentence but the first begins with "Saved", because every class but
/// `auth` kept the record and the credential. The save is a fact; only
/// reachability is in question.
pub fn describe(class: ProbeClass, provider: &str) -> String {
    match class {
        ProbeClass::Auth => {
            format!("Could not reach {provider}: the provider rejected the credential.")
        }
        ProbeClass::Endpoint => format!("Saved, but nothing answered at {provider}."),
        ProbeClass::Model => "Saved. The endpoint did not recognise that model id.".to_string(),
        ProbeClass::Quota => "Saved. The account is out of credit.".to_string(),
        ProbeClass::Timeout => format!("Saved, but {provider} did not answer in time."),
        ProbeClass::Unknown => "Saved, but the check did not complete.".to_string(),
    }
}

/// What to tell the operator when the add was **undone**.
///
/// [`describe`] opens every sentence but one with "Saved", because for a cloud
/// provider every class but `auth` kept the record and the credential. Once
/// [`rolls_back`] can answer true for a second class, that wording becomes a
/// lie in exactly the case it is shown: a local runtime that is not running
/// rolls back, and telling the operator it was saved while no row appears is
/// worse than telling them nothing.
///
/// So the refusal path has its own sentences. Each names the next thing to do,
/// because in every one of these cases there is one.
pub fn describe_refusal(class: ProbeClass, subject: &str) -> String {
    match class {
        ProbeClass::Auth => {
            format!("Could not reach {subject}: the provider rejected the credential.")
        }
        ProbeClass::Endpoint => format!(
            "Nothing answered at {subject}, so it was not connected. Start it and try again."
        ),
        ProbeClass::Timeout => {
            format!("{subject} did not answer in time, so it was not connected.")
        }
        // Not reachable through `rolls_back` today. Answered rather than
        // panicked, because a future class joining the rollback set should
        // degrade to a true sentence rather than to a crash.
        ProbeClass::Model | ProbeClass::Quota | ProbeClass::Unknown => {
            format!("Could not verify {subject}, so it was not connected.")
        }
    }
}

/// Why an endpoint may not be probed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndpointRefusal {
    /// Not a URL this can read a host out of.
    Unparseable,
    /// Something other than `http` or `https`.
    Scheme,
    /// Loopback, and this deployment does not offer local runtimes.
    Loopback,
    /// A link-local or cloud metadata address.
    LinkLocal,
    /// A private or otherwise non-routable address.
    PrivateNetwork,
    /// `http` to somewhere other than this host, with a credential to present.
    Cleartext,
}

impl std::fmt::Display for EndpointRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unparseable => write!(f, "that is not an endpoint address"),
            Self::Scheme => write!(f, "an endpoint must be http or https"),
            Self::Loopback => write!(f, "this host does not offer local model runtimes"),
            Self::LinkLocal => write!(f, "a model endpoint is never on a link-local address"),
            Self::PrivateNetwork => {
                write!(
                    f,
                    "a model endpoint is never on this host's private network"
                )
            }
            Self::Cleartext => {
                write!(
                    f,
                    "a key cannot be sent to an http endpoint off this host — use https"
                )
            }
        }
    }
}

/// Whether loopback is an acceptable probe target on this deployment.
///
/// It is an **explicit allowance**, made because the local-runtime category
/// exists and `ollama` needs it — not a hole left open. A server-side
/// deployment that offers no local runtimes passes `false` and loopback is
/// refused like any other non-routable address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbePolicy {
    /// Whether the local-runtime category is offered here at all.
    pub allow_loopback: bool,
}

/// Whether `url` may be probed.
///
/// Generalising "test this credential against this URL" to company scope creates
/// an authenticated *send a request to an arbitrary address* primitive, which is
/// SSRF-shaped. This is the answer, made explicitly rather than inherited:
///
/// * the scheme must be `http` or `https`;
/// * link-local and cloud metadata addresses (`169.254.0.0/16`, `fe80::/10`) are
///   refused outright — a company's model endpoint is never there, and that
///   range is where a container's credentials live;
/// * other private ranges are refused, because a model endpoint reachable only
///   from inside this host's network is this host's business, not a tenant's;
/// * loopback is allowed only where local runtimes are offered.
///
/// A hostname that is not a literal IP is allowed: resolving it here would be a
/// DNS lookup in a pure function, and a check performed before a resolve is
/// defeated by the resolve changing underneath it anyway. **Apply this to every
/// redirect target too** — a permitted host that redirects to the metadata
/// address is the whole trick.
pub fn check_endpoint(url: &str, policy: ProbePolicy) -> Result<(), EndpointRefusal> {
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(EndpointRefusal::Unparseable);
    };
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return Err(EndpointRefusal::Scheme);
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let host = if let Some(after) = host_port.strip_prefix('[') {
        after.split_once(']').map(|(h, _)| h).unwrap_or(after)
    } else {
        host_port
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(host_port)
    };
    let host = host.trim();
    if host.is_empty() {
        return Err(EndpointRefusal::Unparseable);
    }
    // A name, not a literal. See the doc comment: resolving here would make this
    // impure and would not close the window anyway.
    let Ok(ip) = host.parse::<IpAddr>() else {
        return Ok(());
    };
    check_address(ip, policy)
}

/// [`check_endpoint`], plus the rule that only applies when there is a key.
///
/// **A bearer over plain `http` is the key, in the clear, to everything on the
/// path.** `http` is in the allowed set for the local-runtime category — Ollama
/// documents `http://localhost:11434` and there is no certificate to have — so
/// the scheme cannot simply be narrowed to `https`. The rule that separates the
/// two is the destination, not the scheme: loopback never leaves this host, and
/// anything else with a credential attached does.
///
/// Loopback by **name** as well as by literal, because `localhost` is what the
/// vendor's own documentation prints and it is the address an operator will
/// type. A name is not resolved here for the reason [`check_endpoint`] gives.
///
/// This governs what *we* send. An endpoint stored despite it is still reached
/// by the turn path, which applies no guard of its own — recorded in
/// `docs/modules/inference/provider-contracts.md`.
pub fn check_endpoint_with_credential(
    url: &str,
    policy: ProbePolicy,
    has_credential: bool,
) -> Result<(), EndpointRefusal> {
    check_endpoint(url, policy)?;
    if !has_credential {
        return Ok(());
    }
    let trimmed = url.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Ok(());
    };
    if !scheme.eq_ignore_ascii_case("http") {
        return Ok(());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host_port = authority
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(authority);
    let host = if let Some(after) = host_port.strip_prefix('[') {
        after.split_once(']').map(|(h, _)| h).unwrap_or(after)
    } else {
        host_port
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(host_port)
    };
    let host = host.trim().to_ascii_lowercase();
    let on_this_host = host == "localhost"
        || host.ends_with(".localhost")
        || host
            .parse::<IpAddr>()
            .map(|ip| match ip {
                IpAddr::V4(v4) => v4.is_loopback(),
                IpAddr::V6(v6) => {
                    v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
                }
            })
            .unwrap_or(false);
    if on_this_host {
        return Ok(());
    }
    Err(EndpointRefusal::Cleartext)
}

/// Whether two URLs name the same origin — scheme, host and port.
///
/// **A credentialed request must not follow a redirect off its origin.** `reqwest`
/// strips `Authorization` when the host changes, but it does **not** strip a
/// custom header, and the one non-bearer entry in the catalogue sends the key as
/// `x-api-key`. A provider that can answer `302` could therefore hand an
/// operator's Anthropic key to any host it names. The check is here rather than
/// in the redirect closure so both clients — the probe and the catalogue reader —
/// apply the same rule.
pub fn same_origin(a: &str, b: &str) -> bool {
    fn origin(url: &str) -> Option<(String, String)> {
        let (scheme, rest) = url.trim().split_once("://")?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
        let host_port = authority
            .rsplit_once('@')
            .map(|(_, host)| host)
            .unwrap_or(authority);
        Some((
            scheme.to_ascii_lowercase(),
            host_port.trim().to_ascii_lowercase(),
        ))
    }
    match (origin(a), origin(b)) {
        (Some(left), Some(right)) => left == right,
        // Unparseable on either side is not a match. Refusing to follow costs a
        // catalogue read; following costs the key.
        _ => false,
    }
}

/// The address half of [`check_endpoint`], exposed so a redirect target can be
/// checked after it has been resolved.
pub fn check_address(ip: IpAddr, policy: ProbePolicy) -> Result<(), EndpointRefusal> {
    match ip {
        IpAddr::V4(v4) => check_v4(v4, policy),
        IpAddr::V6(v6) => {
            // An IPv4-mapped address is the same machine wearing a longer name,
            // so it gets the same answer. Checking only the v6 shape here is how
            // `::ffff:169.254.169.254` reaches a metadata service.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return check_v4(mapped, policy);
            }
            if v6.is_loopback() {
                return loopback(policy);
            }
            // fe80::/10 link-local, and fec0::/10 site-local.
            let first = v6.segments()[0];
            if (first & 0xffc0) == 0xfe80 || (first & 0xffc0) == 0xfec0 {
                return Err(EndpointRefusal::LinkLocal);
            }
            // fc00::/7 unique-local.
            if (v6.segments()[0] & 0xfe00) == 0xfc00 {
                return Err(EndpointRefusal::PrivateNetwork);
            }
            if v6 == Ipv6Addr::UNSPECIFIED {
                return Err(EndpointRefusal::PrivateNetwork);
            }
            Ok(())
        }
    }
}

fn check_v4(ip: Ipv4Addr, policy: ProbePolicy) -> Result<(), EndpointRefusal> {
    if ip.is_loopback() {
        return loopback(policy);
    }
    // 169.254.0.0/16 — link-local, and the address every cloud puts its
    // instance credentials behind.
    if ip.is_link_local() {
        return Err(EndpointRefusal::LinkLocal);
    }
    if ip.is_private() || ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() {
        return Err(EndpointRefusal::PrivateNetwork);
    }
    // 100.64.0.0/10, carrier-grade NAT — where a container network often lives.
    let [a, b, ..] = ip.octets();
    if a == 100 && (64..128).contains(&b) {
        return Err(EndpointRefusal::PrivateNetwork);
    }
    Ok(())
}

fn loopback(policy: ProbePolicy) -> Result<(), EndpointRefusal> {
    if policy.allow_loopback {
        Ok(())
    } else {
        Err(EndpointRefusal::Loopback)
    }
}

// ---- the IO half ------------------------------------------------------------
//
// Everything above this line is pure and testable with a string. Below it is the
// one network call this module makes, kept here rather than in a handler so that
// the guard, the caps and the classification travel together: a second caller
// that reached for `reqwest` directly would be a second, unguarded probe.

/// How long a probe may take in total, including connect, TLS and body.
///
/// The operator is watching a dialog spinner while this runs, so it is short. A
/// provider that cannot answer a catalog listing in ten seconds is a `timeout`,
/// which is a non-destructive class — nothing is lost by giving up early.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// How much of a **failure** response body is read before the rest is
/// discarded.
///
/// The body is wanted for one thing only — the wording a vendor puts in an
/// error — and 64 KiB is far more than any of them use. Without a cap, an
/// endpoint that streams indefinitely holds this connection open for the
/// whole timeout and buffers whatever it sent into this process's memory,
/// once per probe.
const PROBE_BODY_CAP: usize = 64 * 1024;

/// How much of a **successful** model-catalog body is read before it is
/// refused as too large (bug KR-L1-01, keys rework issue #2306).
///
/// A real catalog is not an error message: OpenRouter's `/models` alone is
/// ~910 KB for ~600 models, comfortably past the old 64 KiB failure-body cap
/// this used to share. That cap silently truncated the JSON mid-string,
/// [`parse_model_ids`] swallowed the resulting parse failure by design (a
/// body it cannot parse is meant to read as "connected to something that is
/// not a catalog"), and the Add dialog opened on free text reporting a
/// perfectly healthy connection with zero models — the operator's key was
/// never at fault, and nothing said so.
///
/// 16 MiB is the shared budget both catalog readers refuse past: this
/// module's [`probe_get`] for the Add-flow's draft probe, and
/// [`crate::server::inference_models::fetch_catalog`]/`fetch_paged_catalog`
/// for the Edit-flow's live model list — one limit, so an operator meets the
/// same behaviour connecting a provider and editing one, and a catalog that
/// exceeds it is refused with an explicit error rather than silently read as
/// empty. A page of the TinyHumans paged envelope has its own, smaller
/// [`super::paged_catalog::PAGE_BODY_CAP`] (4 MiB) for the same reason at a
/// tighter budget, since it never needs to hold more than one page.
pub(crate) const CATALOG_BODY_CAP: usize = 16 * 1024 * 1024;

/// How many redirects a probe will follow.
///
/// Three rather than `reqwest`'s default ten: a model catalog is a leaf
/// document, and a chain longer than a vendor's http→https plus a host move is
/// not a catalog, it is something worth refusing. **Every hop is re-checked
/// against [`check_endpoint`]** — a permitted host that redirects to the
/// metadata address is the entire SSRF trick, and a policy applied only to the
/// first URL would wave it through.
const PROBE_MAX_REDIRECTS: usize = 3;

/// Whether this deployment permits a probe at a loopback address.
///
/// **An explicit allowance, made because the local-runtime category exists.**
/// `ollama` and `lmstudio` are in the catalogue, an operator may genuinely run
/// one beside the host, and refusing loopback would make that category
/// unreachable. It is one function so that a deployment which drops the category
/// has one place to say so, rather than a boolean threaded through five call
/// sites and defaulted wrong in one of them.
pub fn default_policy() -> ProbePolicy {
    ProbePolicy {
        allow_loopback: !catalogue::LOCAL_RUNTIMES.is_empty(),
    }
}

/// A probe that did not succeed.
///
/// Carries the class *and* the raw upstream text, because they go to two
/// different places: the class decides what happens to the credential and what
/// the operator is told, while the raw text goes to a log and **never** into the
/// copy. It can echo request material — headers, fragments of a key — and the
/// sentence it would land in is one someone screenshots into a ticket.
#[derive(Clone, Debug)]
pub struct ProbeFailure {
    /// What the failure means.
    pub class: ProbeClass,
    /// The upstream text, for a detail or console channel only.
    pub raw: String,
    /// Whether this is specifically a model-catalog body that ran past
    /// [`CATALOG_BODY_CAP`] (bug KR-L1-01) — set only by
    /// [`Self::catalog_too_large`]. A caller that needs to say "the model
    /// list could not be read", rather than [`describe`]'s generic `Unknown`
    /// wording, checks this rather than re-deriving it from `raw`, which is
    /// upstream/log text and never meant to be pattern-matched on.
    pub truncated: bool,
}

impl ProbeFailure {
    /// Classifies a raw error string.
    fn from_raw(raw: String) -> Self {
        Self {
            class: classify(&raw),
            raw,
            truncated: false,
        }
    }

    /// Classifies on one string and remembers another.
    ///
    /// The two are different because **this probe's own URL ends in `/models`**.
    /// Interpolating it into the text the classifier reads makes every single
    /// probe failure contain the word "model", so a refused connection to
    /// `http://127.0.0.1:9/v1/models` classified as a missing *model id* and sent
    /// the operator off to check a model they never typed. The URL is worth
    /// having in a log and is poison in a classifier input.
    fn classified_as(classify_on: &str, raw: String) -> Self {
        Self {
            class: classify(classify_on),
            raw,
            truncated: false,
        }
    }

    /// A refusal by the SSRF guard, which is an endpoint fact rather than a
    /// credential one — so it keeps the key, like every class but `auth`.
    fn refused(refusal: EndpointRefusal) -> Self {
        Self {
            class: ProbeClass::Endpoint,
            raw: refusal.to_string(),
            truncated: false,
        }
    }

    /// A page of the TinyHumans paged catalog that connected but did not parse
    /// (keys rework, issue #2306, slice 2a).
    ///
    /// Classified `Unknown`, never `Auth`: the request reached the endpoint and
    /// got a 2xx with a body this parser could not read, which says nothing
    /// about the credential. `ProbeClass::Auth` is the one class that rolls a
    /// key back on add, so a parse failure must never produce it — a page the
    /// proxy answers oddly is not proof the key is wrong.
    fn unreadable(raw: String) -> Self {
        Self {
            class: ProbeClass::Unknown,
            raw,
            truncated: false,
        }
    }

    /// A model-catalog body that ran past [`CATALOG_BODY_CAP`] (bug
    /// KR-L1-01, keys rework issue #2306).
    ///
    /// `Unknown`, exactly like [`Self::unreadable`] and for the same reason:
    /// a catalog too large to read says nothing about the credential, so it
    /// must never roll one back. Distinguished from an ordinary `Unknown` by
    /// [`Self::truncated`] so the caller can say "the model list could not
    /// be read" specifically, instead of the generic "the check did not
    /// complete" — a real, valid, hundreds-of-models catalog is not the same
    /// failure as a check that never got an answer at all.
    fn catalog_too_large(raw: String) -> Self {
        Self {
            class: ProbeClass::Unknown,
            raw,
            truncated: true,
        }
    }
}

/// Applies a provider's credential to a request in the style that provider's
/// **native** API expects.
///
/// One helper, three callers (this probe, the catalog reader, the per-provider
/// test), because the alternative is three places to be wrong and only one of
/// them reachable from any given bug report.
///
/// The `anthropic-version` header is not optional decoration: Anthropic's native
/// API rejects a request without it as **malformed** — a `400`, not a `401`.
/// That is the diagnostic that distinguishes this from a bad key, and it is why
/// the symptom was a 400 on a key that was perfectly good.
///
/// Verified against Anthropic's own documentation
/// (`platform.claude.com/docs/en/api/models/list`), whose curl example is
/// exactly `-H 'anthropic-version: 2023-06-01' -H "X-Api-Key: …"`.
pub fn apply_auth(
    request: reqwest::RequestBuilder,
    auth: catalogue::AuthStyle,
    credential: Option<&str>,
) -> reqwest::RequestBuilder {
    let key = match credential.map(str::trim).filter(|k| !k.is_empty()) {
        Some(key) => key,
        // Nothing to present. A keyless local runtime is the ordinary case, and
        // sending an empty header would be worse than sending none.
        None => return request,
    };
    match auth {
        catalogue::AuthStyle::None => request,
        catalogue::AuthStyle::Bearer => request.bearer_auth(key),
        catalogue::AuthStyle::Anthropic => request
            .header("x-api-key", key)
            .header("anthropic-version", catalogue::ANTHROPIC_VERSION),
    }
}

/// Asks `{base_url}/models` what the endpoint serves.
///
/// This is the same cheap, read-only call the model picker needs anyway, which
/// is why it is the probe: connecting a provider and listing its models are the
/// same question asked twice, and a heavier "send a real completion" check would
/// charge the operator for the privilege of finding out their key works.
///
/// Returns the model ids on success. On failure the error is **classified**, and
/// only [`ProbeClass::Auth`] means the credential should be rolled back — see
/// the module header for why the naive "roll everything back" answer destroys
/// valid keys.
///
/// `shape` (keys rework, issue #2306, slice 2a) picks the envelope this reads:
/// [`catalogue::CatalogShape::OpenAi`] is the single-response body this
/// function always read; [`catalogue::CatalogShape::PagedEnvelope`] pages the
/// TinyHumans proxy's `{success, data:{data,total,limit,offset}}` shape to
/// `total`, via [`super::paged_catalog`].
pub async fn probe_models(
    base_url: &str,
    credential: Option<&str>,
    auth: catalogue::AuthStyle,
    policy: ProbePolicy,
    shape: catalogue::CatalogShape,
) -> Result<Vec<String>, ProbeFailure> {
    check_endpoint_with_credential(
        base_url,
        policy,
        credential.is_some_and(|c| !c.trim().is_empty()),
    )
    .map_err(ProbeFailure::refused)?;
    let base = base_url.trim().trim_end_matches('/');
    // No credential here to scope a catalogue *by*, but the catalogue's shape
    // parameters apply regardless: without them OpenRouter answers text-only and
    // caps at 500, so the picker this probe populates silently has no vision
    // model in it. See `catalogue::catalog_query`.
    let url = format!("{base}/models{}", catalogue::catalog_query(base));
    // What the failure text is allowed to say (`raw` reaches a host log, and a
    // log is disk — so an endpoint carrying userinfo must not be written into
    // one verbatim) is computed per request inside `probe_get` now, since a
    // paged read makes one per page rather than once here.

    // The redirect policy is where the guard earns its keep. `reqwest` resolves
    // and connects on our behalf, so the only place a redirect target can be
    // inspected is here, before the next request goes out.
    let origin = url.clone();
    let credentialed = credential.is_some_and(|c| !c.trim().is_empty());
    let redirect_policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= PROBE_MAX_REDIRECTS {
            return attempt.stop();
        }
        // A credentialed request stays on its origin. `reqwest` drops
        // `Authorization` across hosts but keeps a custom header, and the
        // catalogue's one non-bearer entry sends the key as `x-api-key`.
        if credentialed && !same_origin(&origin, attempt.url().as_str()) {
            return attempt.stop();
        }
        match check_endpoint(attempt.url().as_str(), policy) {
            Ok(()) => attempt.follow(),
            // `stop` rather than `error`: the caller then sees the redirect's
            // own status, which classifies as an endpoint problem — which is
            // what it is. Either way the request is never sent.
            Err(_) => attempt.stop(),
        }
    });

    let client = reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        .redirect(redirect_policy)
        .build()
        .map_err(|e| ProbeFailure::from_raw(format!("could not build the probe client: {e}")))?;

    // Keys rework (#2306), slice 2a: the TinyHumans proxy pages, and a page is
    // its own GET with its own success/failure classification — `probe_get`
    // below is what both this branch and the plain OpenAI-shaped read call.
    // `origin` (above) is the first page's URL, and every later page shares
    // it, so the same-origin redirect guard still holds across the loop.
    if shape == catalogue::CatalogShape::PagedEnvelope {
        let mut collector = super::paged_catalog::Collector::default();
        loop {
            let page_url = format!(
                "{base}{}",
                super::paged_catalog::page_path(collector.offset())
            );
            let body = probe_get(
                &client,
                &page_url,
                auth,
                credential,
                super::paged_catalog::PAGE_BODY_CAP,
            )
            .await?;
            let page = super::paged_catalog::parse_page(&body).map_err(|e| {
                ProbeFailure::unreadable(format!("{}: {e}", catalogue::redact_endpoint(&page_url)))
            })?;
            if !matches!(collector.push(page), super::paged_catalog::NextPage::At(_)) {
                break;
            }
        }
        return Ok(collector.finish().into_iter().map(|e| e.id).collect());
    }

    let body = probe_get(&client, &url, auth, credential, CATALOG_BODY_CAP).await?;
    Ok(parse_model_ids(&body))
}

/// One GET, classified on failure. Shared by the plain OpenAI-shaped read and
/// every page of the TinyHumans paged read (keys rework, issue #2306, slice
/// 2a) — the request, transport-error and HTTP-failure handling used to live
/// inline in [`probe_models`]; extracted so a page is not a second copy of it.
///
/// `success_cap` bounds a **successful** body — [`CATALOG_BODY_CAP`] for the
/// plain OpenAI-shaped read, [`super::paged_catalog::PAGE_BODY_CAP`] for one
/// page of the TinyHumans envelope; a failure body always reads at the
/// smaller [`PROBE_BODY_CAP`], because nothing needs more of an error to
/// classify it. A success body that runs past `success_cap` is refused
/// outright (bug KR-L1-01) rather than parsed truncated.
async fn probe_get(
    client: &reqwest::Client,
    url: &str,
    auth: catalogue::AuthStyle,
    credential: Option<&str>,
    success_cap: usize,
) -> Result<String, ProbeFailure> {
    let named = catalogue::redact_endpoint(url);
    // The one non-bearer entry in the whole catalogue. A probe that assumed one
    // auth style would fail exactly one provider — the one people try first —
    // and would classify the result as `auth`, deleting a perfectly good key.
    let request = apply_auth(client.get(url), auth, credential);

    let response = request.send().await.map_err(|e| {
        // Classified on the condition alone; the full error, URL and all, is
        // kept for the log. See `ProbeFailure::classified_as`.
        ProbeFailure::classified_as(transport_condition(&e), format!("{named}: {e}"))
    })?;
    let status = response.status();
    if !status.is_success() {
        let (body, _truncated) = read_capped_to(response, PROBE_BODY_CAP).await;
        // The body is included in the string the classifier reads, and only
        // there: vendors put "invalid api key" and "model not found" in the
        // body rather than the reason phrase, so classifying on the status
        // alone would read every one of them as `unknown`. A failure body
        // running past its (small) cap is not reported as truncated — an
        // error message is never legitimately this large, and the cap here
        // exists only to bound a misbehaving endpoint, not to accommodate one.
        let classified = build_failure_text(status, body.trim());
        // The reason phrase is for a human reading the log, and stays out of the
        // text above. See `build_failure_text`.
        // The body goes to a log, and it can echo the request back. A legacy
        // endpoint with userinfo made `reqwest` send it as `Authorization:
        // Basic`, and an upstream 4xx that repeats its headers or credentials
        // would put that password on disk (Codex review on #2281). Scrubbed of
        // exactly what this request carried because of the endpoint.
        let detail = format!(
            "{named}: {} {}: {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("error"),
            scrub_endpoint_credential(url, body.trim())
        );
        return Err(ProbeFailure::classified_as(&classified, detail));
    }
    // Bug KR-L1-01: a body that ran past `success_cap` is refused outright
    // rather than handed to the parser truncated mid-string — the parser
    // cannot tell "this JSON is malformed" from "this JSON was cut off", and
    // treating the second as the first is how a real, valid, hundreds-of-
    // models catalog used to read as a healthy connection with zero models.
    let (body, truncated) = read_capped_to(response, success_cap).await;
    if truncated {
        return Err(ProbeFailure::catalog_too_large(format!(
            "{named}: the model list is larger than the {}-byte cap",
            success_cap
        )));
    }
    Ok(body)
}

/// The text [`classify`] reads for an HTTP failure: the status code and the
/// vendor's body, and **nothing this module wrote itself**.
///
/// The reason phrase is deliberately absent. It used to be here —
/// `"{code} {reason}: {body}"` — and it is how a guard that was written to
/// require vendor wording came to be satisfied by our own: `canonical_reason()`
/// for 403 is the literal string `Forbidden`, which the auth branch tested for,
/// so every 403 from every provider classified as a rejected credential and
/// deleted the operator's key whatever the body said. The same trap is set for
/// any wrapper text containing `unauthorized`, `authentication` or `key`, which
/// is why the rule is now "the vendor's words or nothing".
///
/// The status code stays, because `401` genuinely is a statement about the
/// credential and several vendors send it with an empty body.
fn build_failure_text(status: reqwest::StatusCode, body: &str) -> String {
    format!("{}: {}", status.as_u16(), body)
}

/// The condition a transport failure classifies on.
///
/// Two problems with handing [`classify`] the error's own `Display`. It says
/// "error sending request for url (...)" and buries the cause, so a DNS failure
/// and a timeout read identically — and it **contains the URL**, which for this
/// probe always ends in `/models`, so every failure would carry the word
/// "model". Naming the condition in a short fixed phrase solves both.
fn transport_condition(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        return "timeout";
    }
    if error.is_connect() {
        // Covers DNS failure, connection refused and a TLS handshake that never
        // completed. All three are the same answer to the operator: nothing
        // usable is at that address.
        return "connection refused";
    }
    if error.is_redirect() {
        // The guard stopped the chain, or it was too long. Either way the
        // endpoint did not serve a catalog where it said it would.
        return "redirect not followed: unreachable";
    }
    "the check did not complete"
}

/// `text` with every credential removed that a request to `endpoint` carried
/// **because of the endpoint's own userinfo**.
///
/// `reqwest` lifts `user:password@` out of a URL and sends it as `Authorization:
/// Basic base64(user:password)`, percent-decoded first. So three forms can come
/// back in a response body: the password as written in the URL, the password
/// decoded, and the Basic token. Each is replaced with
/// [`REDACTED_USERINFO`](catalogue::REDACTED_USERINFO), both padded and unpadded.
///
/// **A username with no password is the credential.** `http://sk-secret@host`
/// is how a token gets pasted into a URL, and `reqwest` sends it as
/// `Basic base64(sk-secret:)` — so it is scrubbed as written and decoded, like a
/// password (Codex review on #2281). Beside a password the username is an
/// account name, and is left so an error naming the account still reads.
fn scrub_endpoint_credential(endpoint: &str, text: &str) -> String {
    let Ok(parsed) = url::Url::parse(endpoint.trim()) else {
        return text.to_string();
    };
    let username = percent_decode(parsed.username());
    let password = parsed.password();
    if username.is_empty() && password.is_none() {
        return text.to_string();
    }
    let decoded = password.map(percent_decode).unwrap_or_default();
    let token = base64_standard(format!("{username}:{decoded}").as_bytes());
    let mut secrets = vec![token.trim_end_matches('=').to_string(), token];
    match password {
        Some(raw) => {
            secrets.push(raw.to_string());
            secrets.push(decoded);
        }
        None => {
            secrets.push(parsed.username().to_string());
            secrets.push(username);
        }
    }
    secrets.retain(|secret| !secret.is_empty());
    // Longest first, so a shorter secret never splits a longer one it sits in.
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets.dedup();
    let mut out = text.to_string();
    for secret in &secrets {
        out = out.replace(secret.as_str(), catalogue::REDACTED_USERINFO);
    }
    out
}

/// Percent-decodes `s` the way `reqwest` decodes URL userinfo; an invalid
/// escape is kept as written.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(high), Some(low)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push((high * 16 + low) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Standard padded base64, for matching a Basic token without a dependency this
/// crate only takes behind a feature.
fn base64_standard(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(chunk[0]) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Reads at most `cap` bytes, discarding the rest.
///
/// Chunk by chunk rather than `text()`, because `text()` trusts the endpoint to
/// stop sending. A `Content-Length` header is not a promise either — it is
/// whatever the far side wrote. `cap` used to be fixed at [`PROBE_BODY_CAP`];
/// it is now a parameter so a successful TinyHumans catalog page (keys rework,
/// slice 2a) can read up to [`super::paged_catalog::PAGE_BODY_CAP`] while a
/// failure body still reads at the smaller, fixed cap.
/// Reads at most `cap` bytes, and says whether there was more.
///
/// The second element is `true` when the body kept sending after `cap` bytes
/// had already arrived — the signal [`probe_get`] needs to tell "this body is
/// too large to trust" (bug KR-L1-01) apart from "this body ended, and is not
/// what we hoped for", which are different failures with different remedies.
/// Detected by reading one chunk past the cap rather than stopping exactly at
/// it: a body that ends its stream precisely at `cap` bytes is complete, not
/// truncated, and the two are indistinguishable without that one extra read.
async fn read_capped_to(mut response: reqwest::Response, cap: usize) -> (String, bool) {
    let mut buf: Vec<u8> = Vec::new();
    let mut truncated = false;
    while buf.len() <= cap {
        match response.chunk().await {
            Ok(Some(chunk)) => buf.extend_from_slice(&chunk),
            // A body that stops mid-stream is still worth classifying on what
            // did arrive — the status code is usually the whole signal anyway.
            Ok(None) | Err(_) => break,
        }
        if buf.len() > cap {
            truncated = true;
            break;
        }
    }
    buf.truncate(cap);
    (String::from_utf8_lossy(&buf).into_owned(), truncated)
}

/// The model ids in an OpenAI-compatible `{ "data": [{ "id": ... }] }` body.
///
/// Deliberately forgiving: a probe asks *did this endpoint answer as a model
/// catalog*, and a body it cannot parse is a successful connection to something
/// that is not one. That is still a reachable endpoint, so it is not a failure —
/// the model field simply has nothing to offer, which the console already
/// handles for every endpoint that publishes no catalog.
fn parse_model_ids(body: &str) -> Vec<String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let entries = value
        .get("data")
        .and_then(|d| d.as_array())
        .or_else(|| value.as_array());
    entries
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    entry
                        .get("id")
                        .and_then(|id| id.as_str())
                        .map(|id| id.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "probe_tests_auth_wire.rs"]
mod tests_auth_wire;
#[cfg(test)]
#[path = "probe_tests_catalog.rs"]
mod tests_catalog;
#[cfg(test)]
#[path = "probe_tests_classify.rs"]
mod tests_classify;
#[cfg(test)]
#[path = "probe_tests_ssrf.rs"]
mod tests_ssrf;
