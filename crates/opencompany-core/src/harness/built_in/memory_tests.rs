use super::*;
use std::sync::Mutex;

use crate::ports::types::{ChunkAddr, ChunkHit, ChunkMeta};

#[test]
fn redact_secrets_removes_the_value_but_keeps_the_prose() {
    // A one-time-secret link: the key is stripped, the surrounding sentence
    // (the context an agent needs) is kept.
    assert_eq!(
        redact_secrets("here it is https://ots.example/secret/AbCdEf123456 open it"),
        "here it is https://ots.example/secret/[REDACTED] open it"
    );
    // A bearer token in prose.
    assert_eq!(
        redact_secrets("auth with Bearer sk-verylongsecrettoken please"),
        "auth with Bearer [REDACTED] please"
    );
    // A JWT carries `.` between its base64url segments; the whole
    // credential is consumed, not just the header segment.
    assert_eq!(
        redact_secrets(
            "auth with Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.aSignature0123456789 please"
        ),
        "auth with Bearer [REDACTED] please"
    );
    // An opaque key with base64 punctuation (`+`, `/`, `~`, `=`) is also
    // consumed whole rather than leaking the part after the first such char.
    assert_eq!(
        redact_secrets("auth with Bearer aGVsbG8r/d29ybGQ=andtheRestOfTheKey please"),
        "auth with Bearer [REDACTED] please"
    );
    // A short bearer credential is still a secret: the MCP config accepts
    // any non-empty bearer value, so a token-shaped value like `s3cret`
    // (it contains a digit) is redacted despite being under the prose
    // length floor.
    assert_eq!(
        redact_secrets("auth with Bearer s3cret please"),
        "auth with Bearer [REDACTED] please"
    );
    // A digit-free credential is redacted from six characters on: a real
    // if weak value like `secret` must not persist, while prose words
    // after the marker stay short enough to survive.
    assert_eq!(
        redact_secrets("auth with Bearer secret please"),
        "auth with Bearer [REDACTED] please"
    );
    // The scheme is matched case-insensitively (RFC 9110's auth-scheme
    // ABNF), so lower- and upper-case `bearer` are credentials too.
    assert_eq!(
        redact_secrets("auth with bearer sk-longsecret please"),
        "auth with bearer [REDACTED] please"
    );
    assert_eq!(
        redact_secrets("auth with BEARER sk-longsecret please"),
        "auth with BEARER [REDACTED] please"
    );
    // Lower-case prose survives: `bond` (4) is under the digit-free floor.
    assert_eq!(
        redact_secrets("the bearer bond matures in June"),
        "the bearer bond matures in June"
    );
    // Lower-case "bearer" in its ordinary English sense: a plain word
    // after it is prose, not a credential, and must survive untouched.
    assert_eq!(
        redact_secrets("the standard bearer candidate won the race"),
        "the standard bearer candidate won the race"
    );
    assert_eq!(
        redact_secrets("the ring bearer walked down the aisle"),
        "the ring bearer walked down the aisle"
    );
    // A Markdown-backtick or quote wrapper around a credential (formatted
    // chat) must not defeat the scan.
    assert_eq!(
        redact_secrets("auth with Bearer `sk-verylongsecret` please"),
        "auth with Bearer `[REDACTED]` please"
    );
    assert_eq!(
        redact_secrets("auth with Bearer \"sk-verylongsecret\" please"),
        "auth with Bearer \"[REDACTED]\" please"
    );
    // The MCP config keeps the whole trimmed remainder as the bearer value,
    // so a credential can span space-separated fragments — every fragment
    // of the run is redacted, not just the first.
    assert_eq!(
        redact_secrets("auth with Bearer firstpart secondpart please"),
        "auth with Bearer [REDACTED] [REDACTED] please"
    );
    // Trailing prose after a single-token credential stays readable: a
    // short fragment like "please" (6) is under the continuation floor.
    assert_eq!(
        redact_secrets("auth with Bearer sk-abc123 please"),
        "auth with Bearer [REDACTED] please"
    );
    // No marker: borrowed through untouched, no allocation.
    assert!(matches!(
        redact_secrets("nothing secret here"),
        std::borrow::Cow::Borrowed(_)
    ));
    // Too short after the marker to be a secret: left alone, so ordinary
    // text like "Bearer or not" is not mangled.
    assert_eq!(redact_secrets("Bearer or not"), "Bearer or not");
    // The MCP config trims the value, so extra whitespace between the
    // marker and a credential is legal — and must not leave the credential
    // verbatim because the value scan stopped at the first space.
    assert_eq!(
        redact_secrets("auth with Bearer   sk-longsecret please"),
        "auth with Bearer   [REDACTED] please"
    );
    // Dots do not turn a short prose word into a secret: "key." is still
    // under the digit-free threshold, and plain "token" (no trailing dot)
    // is too.
    assert_eq!(redact_secrets("Bearer key. Please"), "Bearer key. Please");
    assert_eq!(redact_secrets("Bearer token please"), "Bearer token please");
}

/// Minimal in-memory ContextStore for adapter isolation tests.
#[derive(Default)]
struct MockContext {
    chunks: Mutex<Vec<(ChunkAddr, ContextChunk)>>,
}

#[async_trait]
impl ContextStore for MockContext {
    async fn put(&self, _id: &CompanyId, chunk: ContextChunk) -> crate::Result<ChunkAddr> {
        let mut guard = self.chunks.lock().unwrap();
        let addr = ChunkAddr::new(format!("addr-{}", guard.len()));
        guard.push((addr.clone(), chunk));
        Ok(addr)
    }

    async fn list(&self, _id: &CompanyId, prefix: &str) -> crate::Result<Vec<ChunkMeta>> {
        let guard = self.chunks.lock().unwrap();
        Ok(guard
            .iter()
            .filter(|(_, c)| c.label.starts_with(prefix))
            .map(|(addr, c)| ChunkMeta {
                addr: addr.clone(),
                label: c.label.clone(),
                len: c.body.len(),
                // The mock does not model store time; these tests exercise
                // the adapter, not the Brain's freshness stat.
                stored_at_millis: 0,
            })
            .collect())
    }

    async fn peek(
        &self,
        _id: &CompanyId,
        addr: &ChunkAddr,
        _range: Option<std::ops::Range<usize>>,
    ) -> crate::Result<String> {
        let guard = self.chunks.lock().unwrap();
        Ok(guard
            .iter()
            .find(|(a, _)| a == addr)
            .map(|(_, c)| c.body.clone())
            .unwrap_or_default())
    }

    async fn delete(&self, _id: &CompanyId, addr: &ChunkAddr) -> crate::Result<bool> {
        let mut guard = self.chunks.lock().unwrap();
        let before = guard.len();
        guard.retain(|(a, _)| a != addr);
        Ok(guard.len() < before)
    }

    async fn delete_label(
        &self,
        _id: &CompanyId,
        addr: &ChunkAddr,
        label: &str,
    ) -> crate::Result<bool> {
        let mut guard = self.chunks.lock().unwrap();
        let before = guard.len();
        guard.retain(|(a, c)| !(a == addr && c.label == label));
        Ok(guard.len() < before)
    }

    async fn search(
        &self,
        _id: &CompanyId,
        query: &str,
        limit: usize,
    ) -> crate::Result<Vec<ChunkHit>> {
        let guard = self.chunks.lock().unwrap();
        Ok(guard
            .iter()
            .filter(|(_, c)| c.body.contains(query))
            .take(limit)
            .map(|(addr, c)| ChunkHit {
                addr: addr.clone(),
                snippet: c.body.clone(),
                score: 1.0,
            })
            .collect())
    }
}

fn memory() -> OcMemory {
    OcMemory::new(
        CompanyId::new("acme"),
        "ceo",
        Arc::new(MockContext::default()),
    )
}

#[tokio::test]
async fn store_and_get_roundtrip() {
    let mem = memory();
    mem.store("global", "fav_lang", "Rust", MemoryCategory::Core, None)
        .await
        .unwrap();
    let got = mem.get("global", "fav_lang").await.unwrap().unwrap();
    assert_eq!(got.key, "fav_lang");
    assert_eq!(got.content, "Rust");
    assert_eq!(got.namespace.as_deref(), Some("global"));
}

#[tokio::test]
async fn namespacing_isolates_agents_in_the_same_company() {
    let context: Arc<dyn ContextStore> = Arc::new(MockContext::default());
    let ceo = OcMemory::new(CompanyId::new("acme"), "ceo", context.clone());
    let cfo = OcMemory::new(CompanyId::new("acme"), "cfo", context.clone());

    ceo.store("global", "secret", "ceo-only", MemoryCategory::Core, None)
        .await
        .unwrap();
    cfo.store("global", "secret", "cfo-only", MemoryCategory::Core, None)
        .await
        .unwrap();

    // The CFO shares the company + ContextStore but must not see the CEO's
    // namespaced entry. Each agent reads back its *own* value — the CEO's
    // "ceo-only" content is isolated under the CEO's agent scope and never
    // surfaces for the CFO.
    let cfo_got = cfo
        .get("global", "secret")
        .await
        .unwrap()
        .expect("the CFO stored this key itself");
    assert_eq!(cfo_got.content, "cfo-only");
    let ceo_got = ceo
        .get("global", "secret")
        .await
        .unwrap()
        .expect("the CEO stored this key itself");
    assert_eq!(ceo_got.content, "ceo-only");
    assert_eq!(ceo.count().await.unwrap(), 1);
    assert_eq!(cfo.count().await.unwrap(), 1);

    // Both recall methods are scoped the same way: a query both entries
    // match surfaces only the caller's own chunk, and the recalled entry
    // carries the stored namespace and key rather than a substituted
    // default.
    let ceo_hits = ceo
        .recall(
            "only",
            5,
            RecallOpts {
                namespace: Some("global"),
                ..RecallOpts::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(ceo_hits.len(), 1);
    assert_eq!(ceo_hits[0].content, "ceo-only");
    assert_eq!(ceo_hits[0].namespace.as_deref(), Some("global"));
    assert_eq!(ceo_hits[0].key, "secret");

    let cfo_vec = cfo
        .recall_relevant_by_vector("global", "only", 5, 0.0)
        .await
        .unwrap();
    assert_eq!(cfo_vec.len(), 1);
    assert_eq!(cfo_vec[0].1, "cfo-only");
}

#[tokio::test]
async fn recall_matches_by_substring() {
    let mem = memory();
    mem.store(
        "global",
        "note",
        "the quarterly report is ready",
        MemoryCategory::Core,
        None,
    )
    .await
    .unwrap();
    let hits = mem
        .recall("quarterly", 5, RecallOpts::default())
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].content.contains("quarterly"));
    // The recalled entry answers with the stored label's tail, not the
    // requested default: namespace "global" and the real key.
    assert_eq!(hits[0].namespace.as_deref(), Some("global"));
    assert_eq!(hits[0].key, "note");
}

#[tokio::test]
async fn vector_recall_degrades_and_never_errors() {
    let mem = memory();
    mem.store(
        "global",
        "note",
        "vector fallback body",
        MemoryCategory::Core,
        None,
    )
    .await
    .unwrap();
    let hits = mem
        .recall_relevant_by_vector("global", "fallback", 5, 0.5)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].1, "vector fallback body");
}

#[tokio::test]
async fn forget_is_a_no_op_without_a_delete_port() {
    let mem = memory();
    mem.store("global", "k", "v", MemoryCategory::Core, None)
        .await
        .unwrap();
    assert!(!mem.forget("global", "k").await.unwrap());
}

#[tokio::test]
async fn namespace_summaries_group_by_namespace() {
    let mem = memory();
    mem.store("global", "a", "1", MemoryCategory::Core, None)
        .await
        .unwrap();
    mem.store("global", "b", "2", MemoryCategory::Core, None)
        .await
        .unwrap();
    mem.store("user_profile", "c", "3", MemoryCategory::Core, None)
        .await
        .unwrap();
    let summaries = mem.namespace_summaries().await.unwrap();
    assert_eq!(summaries.len(), 2);
    let global = summaries.iter().find(|s| s.namespace == "global").unwrap();
    assert_eq!(global.count, 2);
}
