use super::*;
use crate::ports::types::SecretValue;
use async_trait::async_trait;

/// A secret store with canned values, plus an "unreadable" mode that returns
/// an error to exercise the fail-closed path.
struct FakeSecrets {
    value: Option<String>,
    unreadable: bool,
}

#[async_trait]
impl SecretStore for FakeSecrets {
    async fn get(&self, _company: &CompanyId, _key: &str) -> Result<Option<SecretValue>> {
        if self.unreadable {
            return Err(crate::OpenCompanyError::Store("boom".into()));
        }
        Ok(self.value.clone().map(SecretValue))
    }
    async fn set(&self, _company: &CompanyId, _key: &str, _value: SecretValue) -> Result<()> {
        Ok(())
    }
}

fn company() -> CompanyId {
    CompanyId::new("acme")
}

async fn scrub_with(secrets: FakeSecrets, body: &str) -> ScrubOutcome {
    scrub(
        body,
        &company(),
        &secrets,
        &["github_token".to_string()],
        &["Dana Roe".to_string()],
        &[CharterTerm::new("25.00", "a priced skill")],
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn present_secret_value_aborts() {
    let secrets = FakeSecrets {
        value: Some("ghp_supersecretvalue".into()),
        unreadable: false,
    };
    let out = scrub_with(secrets, "the token ghp_supersecretvalue leaked").await;
    assert!(matches!(out, ScrubOutcome::Aborted { reason } if reason.contains("secret value")));
}

#[tokio::test]
async fn unreadable_secret_store_fails_closed() {
    let secrets = FakeSecrets {
        value: None,
        unreadable: true,
    };
    let out = scrub_with(secrets, "nothing sensitive here").await;
    assert!(matches!(out, ScrubOutcome::Aborted { reason } if reason.contains("unreadable")));
}

#[tokio::test]
async fn high_entropy_token_aborts() {
    let secrets = FakeSecrets {
        value: None,
        unreadable: false,
    };
    // A 40-char random-looking token.
    let out = scrub_with(secrets, "key aB3xQ9zK7mN2pR5tV8wY1cE4gH6jL0oS3uD7fI2n").await;
    assert!(matches!(out, ScrubOutcome::Aborted { reason } if reason.contains("high-entropy")));
}

#[tokio::test]
async fn wallet_private_key_aborts() {
    let secrets = FakeSecrets {
        value: None,
        unreadable: false,
    };
    // A 66-char base58-only token (key-shaped).
    let key = "5Kb8kLf9zgWQnogidDA76MzPL6TsZZY36hWXMssSzNydYXYB9KF2".to_string() + "aBcDeFgHjKmNpQ";
    let out = scrub_with(secrets, &format!("seed {key} here")).await;
    assert!(matches!(out, ScrubOutcome::Aborted { reason } if reason.contains("wallet key")));
}

#[tokio::test]
async fn wallet_address_is_masked() {
    let secrets = FakeSecrets {
        value: None,
        unreadable: false,
    };
    let out = scrub_with(
        secrets,
        "pay sol:9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM now",
    )
    .await;
    match out {
        ScrubOutcome::Ready(body) => {
            assert!(body.contains("sol:…"), "got {body}");
            assert!(!body.contains("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"));
            assert!(body.ends_with("AWWM now") || body.contains("AWWM"));
        }
        other => panic!("expected Ready, got {other:?}"),
    }
}

#[tokio::test]
async fn email_and_name_are_redacted() {
    let secrets = FakeSecrets {
        value: None,
        unreadable: false,
    };
    let out = scrub_with(secrets, "dana@acme.co said Dana Roe was unhappy").await;
    match out {
        ScrubOutcome::Ready(body) => {
            assert!(body.contains("⟨redacted:email⟩"), "got {body}");
            assert!(!body.contains("dana@acme.co"));
            assert!(body.contains("⟨redacted:name⟩"));
            assert!(!body.contains("Dana Roe"));
        }
        other => panic!("expected Ready, got {other:?}"),
    }
}

#[tokio::test]
async fn phone_is_redacted() {
    let secrets = FakeSecrets {
        value: None,
        unreadable: false,
    };
    let out = scrub_with(secrets, "call +1 (415) 555-2671 today").await;
    match out {
        ScrubOutcome::Ready(body) => {
            assert!(body.contains("⟨redacted:phone⟩"), "got {body}");
            assert!(!body.contains("555-2671"));
        }
        other => panic!("expected Ready, got {other:?}"),
    }
}

#[tokio::test]
async fn charter_term_becomes_structural() {
    let secrets = FakeSecrets {
        value: None,
        unreadable: false,
    };
    let out = scrub_with(secrets, "we charge 25.00 for audits").await;
    match out {
        ScrubOutcome::Ready(body) => {
            assert!(body.contains("a priced skill"), "got {body}");
            assert!(!body.contains("25.00"));
        }
        other => panic!("expected Ready, got {other:?}"),
    }
}

#[tokio::test]
async fn clean_body_is_ready_byte_exact() {
    let secrets = FakeSecrets {
        value: None,
        unreadable: false,
    };
    let body = "The invoice route returned the wrong total.";
    let out = scrub_with(secrets, body).await;
    assert_eq!(out, ScrubOutcome::Ready(body.to_string()));
}
