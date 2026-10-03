//! What reading one page's `page.toml` produced, kept apart from "there is no
//! manifest" so a failed read is never reported, listed or rewritten as an
//! empty one.

use super::{CompanyPages, MANIFEST_NAME, PageManifest, store_reason};
use crate::ports::workspace::WorkspaceNode;

/// The four outcomes of resolving a page's manifest.
#[derive(Clone, Debug)]
pub(super) enum ManifestRead {
    /// The manifest exists and parsed.
    Found(PageManifest),
    /// The page has no manifest node.
    Absent,
    /// The manifest exists but its body is not a valid manifest.
    Unparseable(String),
    /// The manifest exists but its body could not be read.
    Unreadable(String),
}

impl ManifestRead {
    /// The manifest to display: the parsed one, or a slug-titled default for
    /// an absent manifest. `None` when the real manifest is unknown.
    pub(super) fn known(&self, slug: &str) -> Option<PageManifest> {
        match self {
            Self::Found(manifest) => Some(manifest.clone()),
            Self::Absent => Some(PageManifest {
                title: slug.to_string(),
                ..Default::default()
            }),
            Self::Unparseable(_) | Self::Unreadable(_) => None,
        }
    }

    /// The `pages_list` line for a manifest whose contents are unknown.
    pub(super) fn list_line(&self, slug: &str) -> Option<String> {
        match self {
            Self::Unparseable(_) => Some(format!(
                "- {slug}: \"{slug}\" ({MANIFEST_NAME} could not be parsed — title shown is the \
                 slug; description, icon and nav visibility are unknown)\n"
            )),
            Self::Unreadable(reason) => Some(format!(
                "- {slug}: (manifest could not be read: {reason} — its title, description and \
                 visibility are unknown, not empty)\n"
            )),
            Self::Found(_) | Self::Absent => None,
        }
    }

    /// The `pages_read` header for a manifest whose contents are unknown.
    pub(super) fn read_header(&self, slug: &str) -> Option<String> {
        match self {
            Self::Unparseable(err) => Some(format!(
                "Page `{slug}`: {MANIFEST_NAME} could not be parsed ({err}), so its title, \
                 description, icon and nav visibility are unknown. pages_write will not change \
                 this manifest unless you pass a `title` to replace it.\n"
            )),
            Self::Unreadable(reason) => Some(format!(
                "Page `{slug}`: its manifest could not be read ({reason}). Title, description, \
                 icon and nav visibility are unknown — not empty. Do not rewrite it with \
                 pages_write until it reads.\n"
            )),
            Self::Found(_) | Self::Absent => None,
        }
    }

    /// The `pages_write` refusal when the manifest cannot be safely rewritten.
    pub(super) fn write_refusal(&self, slug: &str, title_given: bool) -> Option<String> {
        match self {
            Self::Unreadable(reason) => Some(format!(
                "Could not read page `{slug}`'s manifest: {reason}. Nothing was written — writing \
                 now would replace its title, description, icon and visibility with defaults. \
                 Try again; do not recreate the page under another slug."
            )),
            Self::Unparseable(err) if !title_given => Some(format!(
                "`{slug}`'s {MANIFEST_NAME} could not be parsed ({err}), so its current \
                 description, icon and visibility are unknown. Nothing was written. To replace \
                 the manifest, call pages_write again with `title` (and any description, icon, \
                 nav_visible you want) — fields you omit will be reset."
            )),
            _ => None,
        }
    }
}

impl CompanyPages {
    /// Reads and parses `node`, the page's manifest node if it has one.
    pub(super) async fn read_manifest(&self, node: Option<&WorkspaceNode>) -> ManifestRead {
        let Some(node) = node else {
            return ManifestRead::Absent;
        };
        match self.store.read(&self.company, &node.id).await {
            Ok(Some((_, body))) => match toml::from_str(&body) {
                Ok(manifest) => ManifestRead::Found(manifest),
                Err(e) => {
                    tracing::warn!(node = %node.id, error = %e, "[pages] manifest did not parse");
                    ManifestRead::Unparseable(e.message().to_string())
                }
            },
            Ok(None) => {
                tracing::warn!(node = %node.id, "[pages] manifest vanished while being read");
                ManifestRead::Unreadable("it disappeared while being read".to_string())
            }
            Err(e) => ManifestRead::Unreadable(store_reason(&e)),
        }
    }
}
