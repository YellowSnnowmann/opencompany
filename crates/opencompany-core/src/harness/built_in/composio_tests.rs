use super::*;

// Used by the resolver tests below; the module body itself resolves its
// credential through `company::composio::resolve_credential`.
use crate::company::company_key;
use crate::ports::types::SecretValue;

/// The bearer a config would present right now.
async fn token_of(config: &TenantComposio) -> Option<String> {
    config.current_token().await.expect("resolves")
}

fn config_with(credential: Credential) -> Option<TenantComposio> {
    Some(TenantComposio::new(
        "https://api.tinyhumans.ai",
        credential,
        vec!["gmail".to_string()],
    ))
}

#[path = "composio_tests_part1.rs"]
mod tests_part1;
#[path = "composio_tests_part2.rs"]
mod tests_part2;
