//! Company-derived memory namespaces — the tenant-isolation invariant.
//!
//! The three memory ports take `&CompanyId` as an explicit first argument, and
//! that argument is a *compiler-enforced* isolation guarantee: a call site
//! cannot reach company B's facts while holding company A's id, because there is
//! nowhere to put the wrong value. [`MemoryProvider`] has no such argument. It
//! has `namespace: &str`, and a missing or wrong prefix is a silent cross-tenant
//! leak that nothing in the type system catches. That gets worse with a hosted
//! engine, where the namespace string is the only thing keeping tenants apart
//! inside somebody else's database.
//!
//! So this module reintroduces the guarantee the contract gives up.
//! [`Namespace`] is a newtype whose only constructors are `pub(super)`, wrapping
//! a string nothing outside [`crate::store::memory`] can produce, read, or
//! forge. Every namespace originates from
//! [`Namespace::company_root`], which takes a `&CompanyId` — so the *only* way
//! to name a namespace is to already hold the company whose namespace it is.
//!
//! [`MemoryProvider`]: tinymemory_api::provider::MemoryProvider

use sha2::{Digest, Sha256};

use crate::ports::CompanyId;

/// Root segment prefixing every namespace this host mints.
///
/// Present so a hosted engine shared with other tenants of that engine — a
/// Supermemory or Mem0 workspace that is not exclusively ours — cannot collide
/// with namespaces some other product wrote into the same account.
const ROOT: &str = "oc";

/// The namespace segment holding provisional working-out.
///
/// The scratch firewall is not an exclusion rule written against this constant —
/// nothing anywhere matches on it. Each durable facade narrows recall to *its
/// own* namespace and drops any hit reported outside it
/// (`super::facades::Bound::recall`), and scratch is a sibling of all of them,
/// so it is unreachable by construction rather than by being filtered out.
///
/// That is the stronger arrangement: an exclusion list fails open when a new
/// provisional scope is added and nobody updates the list, whereas positive
/// containment fails closed — an unrecognised namespace simply is not contained.
/// It does depend on no scope being nested inside another, which is what
/// [`Scope`] being a closed enum makes checkable.
///
/// Named rather than inlined only so the segment is spelled once.
pub(super) const SCRATCH_SEGMENT: &str = "scratch";

/// Which partition of a company's memory a namespace addresses.
///
/// A closed enum rather than a string: adding a partition is a deliberate edit
/// here, and no caller can invent one. The scratch firewall depends on that —
/// it can only be sound if the set of namespaces is enumerable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Scope {
    /// The operator's hand-curated facts (`FactStore`).
    Facts,
    /// The brain's compressed cycle traces (`MemoryStore`), live set.
    Traces,
    /// Traces evicted from the live set. The contract has no archive tier, so
    /// `evict` moves entries here rather than calling `forget` — see
    /// [`super::facades::ProviderMemoryStore::evict`].
    Archive,
    /// Task results, kept apart from traces so `recent_traces` need not filter.
    TaskResults,
    /// Content-addressed context chunks (`ContextStore`).
    Context,
    /// Provisional working-out, unreachable from durable recall.
    Scratch,
    /// One agent's private partition.
    Agent(String),
    /// One desk's shared partition.
    Desk(String),
}

impl Scope {
    /// The path segment for this scope.
    fn segment(&self) -> String {
        match self {
            Self::Facts => "facts".to_string(),
            Self::Traces => "traces".to_string(),
            Self::Archive => "archive".to_string(),
            Self::TaskResults => "task-results".to_string(),
            Self::Context => "context".to_string(),
            Self::Scratch => SCRATCH_SEGMENT.to_string(),
            // `scope_member`, not `sanitize_segment`: the member has to be
            // injective. Sanitizing alone maps `a:b` and `a/b` onto one
            // segment, which would point two agents at a single namespace.
            Self::Agent(id) => format!("agent/{}", scope_member(id)),
            Self::Desk(id) => format!("desk/{}", scope_member(id)),
        }
    }
}

/// A memory namespace derived from a [`CompanyId`].
///
/// Deliberately opaque: no `From<String>`, no `Deref`, no public constructor,
/// and [`Namespace::as_str`] is `pub(super)` so even reading the string is
/// confined to this module tree. Code outside cannot name a namespace, which is
/// what makes a cross-company leak unrepresentable rather than merely untested.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Namespace(String);

impl Namespace {
    /// Derives a company's root namespace.
    ///
    /// The only entry point. Everything else is a [`Namespace::child`] of a
    /// value that came from here, so every namespace in the process is rooted in
    /// some `CompanyId` the caller was holding.
    pub(super) fn company_root(company: &CompanyId) -> Self {
        Self(format!("{ROOT}/{}", workspace_segment(company.as_ref())))
    }

    /// Derives a child namespace for one partition of this company's memory.
    pub(super) fn child(&self, scope: &Scope) -> Self {
        Self(format!("{}/{}", self.0, scope.segment()))
    }

    /// The namespace string, for handing to the provider.
    pub(super) fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether `candidate` — a namespace string that came *back* from a driver —
    /// is inside this namespace.
    ///
    /// Used to check what a driver returned rather than what we asked it for. A
    /// remote engine is somebody else's code answering our query, and a driver
    /// that over-returns (ignoring the namespace filter, or honouring it
    /// loosely) would otherwise hand one tenant another's entries. The boundary
    /// check is `/`-aware so `oc/acme-1` never matches `oc/acme-10`.
    pub(super) fn contains(&self, candidate: &str) -> bool {
        candidate == self.0
            || candidate
                .strip_prefix(&self.0)
                .is_some_and(|rest| rest.starts_with('/'))
    }
}

/// Maps a raw company id to a path-safe, **collision-resistant** namespace
/// segment.
///
/// Sanitizing alone is not injective: mapping every character outside
/// `[A-Za-z0-9-_]` to `_` collapses `acme:1`, `acme/1`, and `acme_1` onto one
/// segment — three companies sharing one namespace, reading each other's memory.
/// That is the exact failure this whole module exists to prevent, so a suffix
/// derived from a stable hash of the **full raw** id is always appended: when
/// two sanitized prefixes collide their raw ids still differ, and so do their
/// hashes.
///
/// "Collision-resistant" rather than "injective", deliberately: any finite-width
/// hash maps unbounded input onto a fixed range, so distinct outputs cannot be
/// *guaranteed*. What can be guaranteed is that finding two ids that collide is
/// computationally infeasible — which is the property that matters, because
/// company ids can be chosen by a caller.
///
/// This mirrors the removed in-pod engine's `workspace_name` (deleted with
/// the `tinycortex` backend in #1568), which solved the same problem for
/// on-disk workspace directories. The two are intentionally separate —
/// that one named a filesystem path, this one names a namespace inside a
/// possibly-remote engine — but the collision argument is identical, and a
/// change to one should prompt a look at the other.
fn workspace_segment(company: &str) -> String {
    let prefix = sanitize_segment(company);
    let suffix = stable_hash_hex(company);
    if prefix.is_empty() {
        format!("h-{suffix}")
    } else {
        format!("{prefix}-{suffix}")
    }
}

/// An agent or desk id as a namespace segment: path-safe, never empty, and
/// collision-resistant.
///
/// Sanitizing alone is not injective, and the consequence is the same one
/// [`workspace_segment`] exists to prevent, one level down: `a:b` and `a/b` both
/// sanitize to `a_b`, so two agents would address a single provider namespace
/// and each could read the other's private partition. That stays inside one
/// company — it is not a cross-tenant leak — but "agent A can read agent B's
/// private memory" is not a property to concede to punctuation.
///
/// So the same construction is used: the sanitized prefix for legibility, plus a
/// suffix derived from the **full raw** id, which differs whenever the raw ids
/// do. An id that sanitizes to nothing keeps the `h-` form rather than an empty
/// segment, so a scope always names a member instead of the scope kind itself.
///
/// Unlike the company segment this is not a security boundary between tenants,
/// but it is cheap, and having one rule for both means a reader does not have to
/// work out which segments are injective and which merely look like it.
fn scope_member(raw: &str) -> String {
    workspace_segment(raw)
}

/// Maps a raw identifier to the path-safe alphabet, without the hash suffix.
///
/// Not injective on its own, and never used on its own: both callers
/// ([`workspace_segment`], and [`scope_member`] through it) append a hash of the
/// raw id. This produces the legible half of a segment, so a namespace reads as
/// `oc/acme-<hash>` rather than as an opaque digest.
fn sanitize_segment(raw: &str) -> String {
    raw.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// SHA-256 over the raw bytes, truncated to 128 bits. Stable across processes
/// and releases, which is what durability needs — a company's namespace must not
/// move under it.
///
/// Cryptographic on purpose. This was FNV-1a, which is fine for a hash map and
/// wrong here: a company id is not operator-only — `POST /api/v1/companies`
/// accepts an explicit one — so an id that collides with another tenant's is
/// something a caller can *choose*, and FNV-1a is trivial to collide by
/// construction. The suffix is the only thing keeping two sanitized ids apart,
/// so it has to resist a deliberate collision, not just an accidental one.
///
/// 128 bits keeps the segment short while putting a chosen collision out of
/// reach.
fn stable_hash_hex(s: &str) -> String {
    let digest = Sha256::digest(s.as_bytes());
    digest[..16].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod test {
    use super::*;

    fn id(raw: &str) -> CompanyId {
        CompanyId::new(raw)
    }

    #[test]
    fn sanitized_collisions_stay_distinct_namespaces() {
        // The whole point: these three sanitize to the same prefix.
        let a = Namespace::company_root(&id("acme:1"));
        let b = Namespace::company_root(&id("acme/1"));
        let c = Namespace::company_root(&id("acme_1"));
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn derivation_is_stable_across_calls() {
        // Durability rests on this: a company's namespace must not move.
        assert_eq!(
            Namespace::company_root(&id("acme")),
            Namespace::company_root(&id("acme"))
        );
    }

    #[test]
    fn an_empty_id_still_yields_a_namespace() {
        let ns = Namespace::company_root(&id(""));
        assert!(ns.as_str().starts_with("oc/h-"), "{}", ns.as_str());
    }

    #[test]
    fn tenant_prefixed_ids_stay_distinct() {
        // Shared-single-DB mode prefixes ids with `<tenant>--`. Two tenants
        // booting the same company template must not share a namespace.
        let a = Namespace::company_root(&id("acme--agentic_software_company"));
        let b = Namespace::company_root(&id("globex--agentic_software_company"));
        assert_ne!(a, b);
    }

    #[test]
    fn children_of_different_companies_never_collide() {
        let a = Namespace::company_root(&id("acme")).child(&Scope::Facts);
        let b = Namespace::company_root(&id("globex")).child(&Scope::Facts);
        assert_ne!(a, b);
    }

    #[test]
    fn scopes_partition_one_company() {
        let root = Namespace::company_root(&id("acme"));
        let scopes = [
            Scope::Facts,
            Scope::Traces,
            Scope::Archive,
            Scope::TaskResults,
            Scope::Context,
            Scope::Scratch,
            Scope::Agent("cto".into()),
            Scope::Desk("eng".into()),
        ];
        let mut seen = std::collections::HashSet::new();
        for scope in &scopes {
            assert!(
                seen.insert(root.child(scope)),
                "two scopes collided: {scope:?}"
            );
        }
    }

    #[test]
    fn agent_ids_that_sanitize_alike_stay_distinct() {
        // `a:b` and `a/b` both sanitize to `a_b`. Merging them would point two
        // agents at one namespace, and each could read the other's private
        // partition.
        let root = Namespace::company_root(&id("acme"));
        let a = root.child(&Scope::Agent("a:b".into()));
        let b = root.child(&Scope::Agent("a/b".into()));
        let c = root.child(&Scope::Agent("a_b".into()));
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }

    #[test]
    fn desk_ids_that_sanitize_alike_stay_distinct() {
        let root = Namespace::company_root(&id("acme"));
        assert_ne!(
            root.child(&Scope::Desk("a:b".into())),
            root.child(&Scope::Desk("a/b".into()))
        );
    }

    #[test]
    fn an_agent_and_a_desk_with_the_same_id_stay_apart() {
        // They differ by the `agent/` vs `desk/` prefix, not by the member, so
        // this holds independently of how the member is derived.
        let root = Namespace::company_root(&id("acme"));
        assert_ne!(
            root.child(&Scope::Agent("ops".into())),
            root.child(&Scope::Desk("ops".into()))
        );
    }

    #[test]
    fn a_member_id_that_sanitizes_to_nothing_still_names_a_member() {
        // Otherwise `child` builds a namespace ending in `/`, naming the scope
        // kind rather than any member of it.
        let root = Namespace::company_root(&id("acme"));
        let empty = root.child(&Scope::Agent(String::new()));
        assert!(!empty.as_str().ends_with('/'), "{}", empty.as_str());
        assert!(empty.as_str().contains("agent/h-"), "{}", empty.as_str());
        // And still distinct from a different unsanitizable id.
        assert_ne!(empty, root.child(&Scope::Agent(":".into())));
    }

    #[test]
    fn member_derivation_is_stable_across_calls() {
        let root = Namespace::company_root(&id("acme"));
        assert_eq!(
            root.child(&Scope::Agent("cto".into())),
            root.child(&Scope::Agent("cto".into()))
        );
    }

    #[test]
    fn contains_is_boundary_aware() {
        // `oc/acme-1` must not swallow `oc/acme-10` — a prefix test without the
        // separator check would hand one company another's entries.
        let root = Namespace("oc/acme-1".to_string());
        assert!(root.contains("oc/acme-1"));
        assert!(root.contains("oc/acme-1/facts"));
        assert!(!root.contains("oc/acme-10"));
        assert!(!root.contains("oc/acme-10/facts"));
        assert!(!root.contains("oc/globex-2/facts"));
    }

    #[test]
    fn a_company_root_never_contains_another_companys_namespace() {
        let a = Namespace::company_root(&id("acme"));
        let b = Namespace::company_root(&id("globex"));
        assert!(!a.contains(b.as_str()));
        assert!(!b.contains(a.as_str()));
    }

    #[test]
    fn agent_scopes_are_nested_under_the_company() {
        let root = Namespace::company_root(&id("acme"));
        let agent = root.child(&Scope::Agent("cto".into()));
        assert!(root.contains(agent.as_str()));
    }
}
