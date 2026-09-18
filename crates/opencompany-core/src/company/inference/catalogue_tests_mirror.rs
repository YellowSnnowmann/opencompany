//! The cross-language console-mirror tests: the TypeScript copy of the
//! catalogue table must list the same providers, hosts and copy
//! strings (split out of `catalogue_tests.rs`).

use super::*;

// ── The cross-language check ────────────────────────────────────────────
//
// The console needs this table too, and TypeScript cannot read a Rust
// `const`. So there are two copies, and the only thing that keeps two
// hand-maintained copies honest is something that fails when they disagree.
// openhuman has the same two copies and no such test, which is listed as a
// defect for exactly that reason; this repo had six copies and two had
// already silently drifted.
//
// The reader below is deliberately strict — one field per line, quoted
// values, no comments inside an entry — and the mirror's header says so. A
// forgiving parser would let the file drift into a shape the test quietly
// stops checking, which is worse than no test because it reads as coverage.

/// The repository root, which is **not** `CARGO_MANIFEST_DIR`.
///
/// The package manifest lives in `crates/opencompany-core/` while `src/`,
/// `tests/` and `frontend/` stayed at the repository root and are reached
/// from it with `../../` paths. So a repo-relative path joined onto
/// `CARGO_MANIFEST_DIR` lands in a directory that does not exist, and these
/// tests failed with "No such file or directory" rather than on anything
/// they meant to check.
///
/// Walking up to the first ancestor that actually holds the console mirror
/// keeps this correct under both that layout and a single root crate,
/// rather than hard-coding a `../..` that one of the two would get wrong.
fn repo_root() -> std::path::PathBuf {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest
        .ancestors()
        .find(|dir| dir.join("frontend/src/inference/catalogue.ts").is_file())
        .unwrap_or(manifest)
        .to_path_buf()
}

fn mirror_source() -> String {
    let path = repo_root().join("frontend/src/inference/catalogue.ts");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("console mirror at {} is unreadable: {e}", path.display()))
}

/// The text between `export const <name> … = [` and the closing `];`.
fn array_body<'a>(src: &'a str, name: &str) -> &'a str {
    let decl = format!("export const {name}");
    let start = src
        .find(&decl)
        .unwrap_or_else(|| panic!("the mirror declares no {name}"));
    let open = src[start..]
        .find('[')
        .unwrap_or_else(|| panic!("{name} is not an array literal"))
        + start
        + 1;
    let close = src[open..]
        .find("\n];")
        .unwrap_or_else(|| panic!("{name} is not terminated by a bare `];`"))
        + open;
    &src[open..close]
}

/// Object literals in an array body, as ordered `(field, value)` lists.
fn object_entries(body: &str) -> Vec<Vec<(String, String)>> {
    let mut entries = Vec::new();
    let mut current: Option<Vec<(String, String)>> = None;
    for line in body.lines() {
        let line = line.trim();
        if line == "{" {
            current = Some(Vec::new());
            continue;
        }
        if line == "}," || line == "}" {
            if let Some(fields) = current.take() {
                entries.push(fields);
            }
            continue;
        }
        let Some(fields) = current.as_mut() else {
            continue;
        };
        let Some((key, value)) = line.split_once(':') else {
            panic!("mirror entry line is not `field: value` — {line:?}");
        };
        let value = value.trim().trim_end_matches(',').trim();
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .unwrap_or(value);
        fields.push((key.trim().to_string(), value.to_string()));
    }
    assert!(
        current.is_none(),
        "an unterminated object literal in the mirror"
    );
    entries
}

/// Bare quoted strings in an array body.
fn string_entries(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| {
            let line = line.trim().trim_end_matches(',');
            line.strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .map(str::to_string)
        })
        .collect()
}

/// A field of a parsed entry, or `None` when the mirror omits it.
fn field<'a>(entry: &'a [(String, String)], name: &str) -> Option<&'a str> {
    entry
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

/// A field the mirror must carry.
fn required<'a>(entry: &'a [(String, String)], name: &str, row: usize) -> &'a str {
    field(entry, name).unwrap_or_else(|| panic!("mirror row {row} has no `{name}`: {entry:?}"))
}

#[test]
fn the_console_mirror_lists_the_same_cloud_providers() {
    let src = mirror_source();
    let entries = object_entries(array_body(&src, "CLOUD_PROVIDERS"));
    assert_eq!(
        entries.len(),
        CLOUD_PROVIDERS.len(),
        "the mirror lists {} cloud providers, Rust lists {}",
        entries.len(),
        CLOUD_PROVIDERS.len()
    );
    for (row, (entry, provider)) in entries.iter().zip(CLOUD_PROVIDERS).enumerate() {
        assert_eq!(
            required(entry, "slug", row),
            provider.slug,
            "row {row} slug"
        );
        assert_eq!(
            required(entry, "label", row),
            provider.label,
            "{}: label",
            provider.slug
        );
        assert_eq!(
            required(entry, "endpoint", row),
            provider.endpoint,
            "{}: endpoint",
            provider.slug
        );
        assert_eq!(
            required(entry, "auth", row),
            provider.auth.as_str(),
            "{}: auth style",
            provider.slug
        );
        assert_eq!(
            field(entry, "keyPlaceholder"),
            provider.key_placeholder,
            "{}: key placeholder",
            provider.slug
        );
    }
}

#[test]
fn the_console_mirror_lists_the_same_local_runtimes() {
    let src = mirror_source();
    let entries = object_entries(array_body(&src, "LOCAL_RUNTIMES"));
    assert_eq!(entries.len(), LOCAL_RUNTIMES.len(), "local runtime count");
    for (row, (entry, runtime)) in entries.iter().zip(LOCAL_RUNTIMES).enumerate() {
        assert_eq!(required(entry, "slug", row), runtime.slug, "row {row} slug");
        assert_eq!(
            required(entry, "label", row),
            runtime.label,
            "{}: label",
            runtime.slug
        );
        assert_eq!(
            field(entry, "defaultEndpoint"),
            runtime.default_endpoint,
            "{}: default endpoint",
            runtime.slug
        );
        assert_eq!(
            required(entry, "needsKey", row),
            if runtime.needs_key { "true" } else { "false" },
            "{}: needs a key",
            runtime.slug
        );
    }
}

#[test]
fn the_console_mirror_lists_the_same_cli_logins() {
    let src = mirror_source();
    let entries = object_entries(array_body(&src, "CLI_LOGINS"));
    assert_eq!(entries.len(), CLI_LOGINS.len(), "CLI login count");
    for (row, (entry, login)) in entries.iter().zip(CLI_LOGINS).enumerate() {
        assert_eq!(
            required(entry, "optionSlug", row),
            login.option_slug,
            "row {row} option slug"
        );
        // The Codex trap: this is the assertion that keeps the console's
        // already-connected check able to match at all.
        assert_eq!(
            required(entry, "storedSlug", row),
            login.stored_slug,
            "{}: stored slug",
            login.option_slug
        );
        assert_eq!(
            required(entry, "label", row),
            login.label,
            "{}: label",
            login.option_slug
        );
        assert_eq!(
            required(entry, "probes", row),
            if login.probes { "true" } else { "false" },
            "{}: probes",
            login.option_slug
        );
    }
}

#[test]
fn the_console_mirror_lists_the_same_azure_hosts() {
    let src = mirror_source();
    let hosts = string_entries(array_body(&src, "AZURE_ENDPOINT_HOSTS"));
    assert_eq!(
        hosts,
        AZURE_ENDPOINT_HOSTS
            .iter()
            .map(|h| h.to_string())
            .collect::<Vec<_>>(),
        "the Azure host lists have diverged — including, possibly, by adding \
             the Foundry serverless hosts to one side"
    );
}

#[test]
fn the_console_mirror_uses_the_same_copy() {
    let src = mirror_source();
    let start = src
        .find("export const COPY = {")
        .expect("the mirror declares no COPY");
    let open = src[start..].find('{').expect("COPY is not an object") + start + 1;
    let close = src[open..]
        .find("\n} as const;")
        .expect("COPY is not terminated by `} as const;`")
        + open;
    let mut pairs = Vec::new();
    for line in src[open..close].lines() {
        let line = line.trim().trim_end_matches(',');
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let Some(value) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
            continue;
        };
        pairs.push((key.trim().to_string(), value.to_string()));
    }
    let lookup = |name: &str| -> String {
        pairs
            .iter()
            .find(|(k, _)| k == name)
            .unwrap_or_else(|| panic!("the mirror's COPY has no `{name}`"))
            .1
            .clone()
    };
    assert_eq!(lookup("groupCloud"), copy::GROUP_CLOUD);
    assert_eq!(lookup("groupLocal"), copy::GROUP_LOCAL);
    assert_eq!(lookup("groupCli"), copy::GROUP_CLI);
    assert_eq!(lookup("placeholderCloud"), copy::PLACEHOLDER_CLOUD);
    assert_eq!(lookup("placeholderLocal"), copy::PLACEHOLDER_LOCAL);
    assert_eq!(lookup("placeholderCli"), copy::PLACEHOLDER_CLI);
    assert_eq!(lookup("helperCloud"), copy::HELPER_CLOUD);
    assert_eq!(lookup("helperLocal"), copy::HELPER_LOCAL);
    assert_eq!(lookup("helperCli"), copy::HELPER_CLI);
    assert_eq!(lookup("detailLocal"), copy::DETAIL_LOCAL);
    assert_eq!(lookup("detailCli"), copy::DETAIL_CLI);
    assert_eq!(pairs.len(), 11, "a copy string was added to one side only");
}
