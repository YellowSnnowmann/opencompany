use super::*;

#[test]
fn pkce_challenge_is_s256_of_verifier() {
    let (verifier, challenge) = gen_pkce();
    assert!(
        (43..=128).contains(&verifier.len()),
        "verifier in PKCE range: {}",
        verifier.len()
    );
    let expected = B64.encode(Sha256::digest(verifier.as_bytes()));
    assert_eq!(challenge, expected);
}

#[test]
fn callback_uri_appends_route_and_trims_trailing_slash() {
    assert_eq!(
        callback_redirect_uri("http://127.0.0.1:8080"),
        "http://127.0.0.1:8080/oauth/mcp/callback"
    );
    assert_eq!(
        callback_redirect_uri("https://acme.example/"),
        "https://acme.example/oauth/mcp/callback"
    );
}

#[test]
fn authorize_url_carries_pkce_and_resource() {
    let url = build_authorize_url(
        "https://as.example/authorize",
        "client-123",
        "http://127.0.0.1:8080/oauth/mcp/callback",
        "challenge-abc",
        "state-xyz",
        "https://mcp.example/mcp",
    )
    .expect("url");
    assert!(url.contains("response_type=code"));
    assert!(url.contains("client_id=client-123"));
    assert!(url.contains("code_challenge=challenge-abc"));
    assert!(url.contains("code_challenge_method=S256"));
    assert!(url.contains("state=state-xyz"));
    // The MCP server URL is the `resource`, percent-encoded.
    assert!(url.contains("resource=https%3A%2F%2Fmcp.example%2Fmcp"));
}

#[test]
fn parse_token_response_extracts_fields() {
    let v = json!({"access_token":"a","refresh_token":"r","expires_in":3600});
    let t = parse_token_response(&v).unwrap();
    assert_eq!(t.access_token, "a");
    assert_eq!(t.refresh_token.as_deref(), Some("r"));
    assert_eq!(t.expires_in, Some(3600));
    // refresh_token / expires_in are optional; access_token is required.
    let minimal = parse_token_response(&json!({"access_token":"x"})).unwrap();
    assert_eq!(minimal.access_token, "x");
    assert!(minimal.refresh_token.is_none());
    assert!(parse_token_response(&json!({"token_type":"bearer"})).is_err());
}

#[test]
fn oauth_material_secret_values_cover_every_token() {
    // The security invariant: access token, refresh token, and client secret
    // all reach the scrubber's known-secret set.
    let material = AuthMaterial::OAuth {
        access_token: "at-secret".into(),
        refresh_token: Some("rt-secret".into()),
        client_id: "cid".into(),
        client_secret: Some("cs-secret".into()),
        token_endpoint: "https://as/token".into(),
        expires_at: 0,
    };
    let secrets = material.secret_values();
    assert!(secrets.contains(&"at-secret".to_string()));
    assert!(secrets.contains(&"rt-secret".to_string()));
    assert!(secrets.contains(&"cs-secret".to_string()));
    // The client id is NOT a secret and must not be in the set.
    assert!(!secrets.contains(&"cid".to_string()));
    assert!(material.is_configured());
}

#[test]
fn needs_refresh_only_fires_near_expiry() {
    let fresh = AuthMaterial::OAuth {
        access_token: "a".into(),
        refresh_token: None,
        client_id: "c".into(),
        client_secret: None,
        token_endpoint: "https://as/token".into(),
        expires_at: now_unix() + 3600,
    };
    assert!(!needs_refresh(&fresh, 60));
    let stale = AuthMaterial::OAuth {
        access_token: "a".into(),
        refresh_token: None,
        client_id: "c".into(),
        client_secret: None,
        token_endpoint: "https://as/token".into(),
        expires_at: now_unix() + 30,
    };
    assert!(needs_refresh(&stale, 60));
    // A non-OAuth material never needs refresh.
    assert!(!needs_refresh(&AuthMaterial::Bearer("t".into()), 60));
}

#[tokio::test]
async fn guard_blocks_non_https_and_local_targets() {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    // Every case here short-circuits before DNS (bad scheme, bad URL, or an
    // IP-literal host), so the test never touches the network.
    // Scheme: plain http is refused outright (SSRF loves cleartext localhost).
    assert!(
        guard_endpoint("http://as.example/token", "token")
            .await
            .is_err()
    );
    // IP-literal hosts in blocked ranges are rejected without DNS.
    assert!(
        guard_endpoint("https://127.0.0.1/token", "token")
            .await
            .is_err()
    );
    assert!(
        guard_endpoint("https://10.0.0.5/register", "registration")
            .await
            .is_err()
    );
    assert!(
        guard_endpoint("https://192.168.1.1/token", "token")
            .await
            .is_err()
    );
    // The cloud metadata endpoint — the canonical SSRF target — is link-local.
    assert!(
        guard_endpoint("https://169.254.169.254/latest/meta-data", "token")
            .await
            .is_err()
    );
    assert!(
        guard_endpoint("https://[::1]/token", "token")
            .await
            .is_err()
    );
    // A syntactically bad URL is a clean rejection, not a panic.
    assert!(guard_endpoint("not a url", "token").await.is_err());

    // The IP-classification helper covers v4 + v6 ranges directly.
    assert!(is_blocked_ip(&IpAddr::V4(Ipv4Addr::new(
        169, 254, 169, 254
    ))));
    assert!(is_blocked_ip(&IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
    assert!(is_blocked_ip(&IpAddr::V6(Ipv6Addr::LOCALHOST)));
    assert!(is_blocked_ip(&IpAddr::V6("fc00::1".parse().unwrap())));
    assert!(is_blocked_ip(&IpAddr::V6("fe80::1".parse().unwrap())));
    // A routable public address passes the classifier.
    assert!(!is_blocked_ip(&IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))));
    assert!(!is_blocked_ip(&IpAddr::V6(
        "2606:2800:220:1::1".parse().unwrap()
    )));
}

#[tokio::test]
async fn refresh_is_noop_without_refresh_token() {
    let material = AuthMaterial::OAuth {
        access_token: "a".into(),
        refresh_token: None,
        client_id: "c".into(),
        client_secret: None,
        token_endpoint: "https://as/token".into(),
        expires_at: 0,
    };
    assert!(refresh(&material).await.is_none());
    // A non-OAuth material is also a no-op.
    assert!(refresh(&AuthMaterial::Bearer("t".into())).await.is_none());
}
