use super::*;

// Env access is serialised on the crate-wide lock in `crate::test_support`,
// not on one local to this module: all of these tests link into a single
// test binary, so a module-local lock would leave every *other*
// env-touching test in it free to race these.

fn smtp_creds() -> SmtpCredentials {
    SmtpCredentials {
        host: "smtp.example.com".into(),
        port: 587,
        security: SmtpSecurity::Starttls,
        username: "user".into(),
        password: SecretValue("hunter2".into()),
        from_name: "Acme".into(),
        from_email: "hi@acme.test".into(),
    }
}

#[test]
fn credentials_are_tagged_by_provider_on_the_wire() {
    let creds = MailCredentials::Smtp(smtp_creds());
    let json = serde_json::to_value(&creds).unwrap();
    // The tag is what lets a stored blob name its own transport.
    assert_eq!(json["provider"], "smtp");
    assert_eq!(json["host"], "smtp.example.com");
    let back: MailCredentials = serde_json::from_value(json).unwrap();
    assert_eq!(back.provider(), MailProvider::Smtp);
    assert_eq!(back.from_email(), "hi@acme.test");
}

#[test]
fn debug_never_prints_the_password() {
    let creds = MailCredentials::Smtp(smtp_creds());
    let rendered = format!("{creds:?}");
    assert!(
        !rendered.contains("hunter2"),
        "the password leaked into Debug: {rendered}"
    );
    // Still identifies itself usefully. `MailProvider`'s derived Debug is
    // the variant name; the lowercase spelling is the serde/Display form.
    assert!(rendered.contains("Smtp"), "unhelpful Debug: {rendered}");
    assert!(rendered.contains("hi@acme.test"));
}

/// The inverse of the guard-rail this used to be.
///
/// Until issue #1770 this test asserted that `SmtpCredentials`' derived
/// `Debug` *did* print its password, and said that when it stopped, the
/// derive had been fixed and `MailCredentials`' hand-written `Debug` could
/// be relaxed. Both of those happened: the password is a `SecretValue`, so
/// the derive is safe at every level and `MailCredentials` now derives too.
/// The assertion is kept, pointing the other way, so that reverting the
/// field to a `String` fails here as well as in `smtp::credential_tests`.
#[test]
fn smtp_credentials_debug_no_longer_leaks_so_containers_may_derive() {
    let rendered = format!("{:?}", smtp_creds());
    assert!(
        !rendered.contains("hunter2"),
        "SmtpCredentials::Debug leaks its password again: {rendered}"
    );
    // Still useful for diagnosis.
    assert!(rendered.contains("smtp.example.com"), "{rendered}");
}

#[test]
fn provider_parses_and_rejects_unknown() {
    assert_eq!("smtp".parse::<MailProvider>().unwrap(), MailProvider::Smtp);
    assert_eq!(
        "  SMTP ".parse::<MailProvider>().unwrap(),
        MailProvider::Smtp
    );
    let err = "ses".parse::<MailProvider>().unwrap_err();
    assert_eq!(err.code(), "config_error");
    // The message should name what IS supported, not just what isn't.
    assert!(format!("{err}").contains("smtp"));
}

#[tokio::test]
async fn recording_sender_captures_the_envelope_sender() {
    let sender = RecordingMailSender::new();
    let creds = MailCredentials::Smtp(smtp_creds());
    sender
        .send(
            &creds,
            &OutboundEmail {
                to: "ada@example.com".into(),
                subject: "hi".into(),
                body: "body".into(),
            },
        )
        .await
        .unwrap();
    let sent = sender.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, "hi@acme.test");
    assert_eq!(sent[0].1.to, "ada@example.com");
}

#[tokio::test]
async fn recording_receiver_returns_queued_batches_in_order() {
    let creds = ImapCredentials {
        host: "h".into(),
        port: 993,
        username: "u".into(),
        password: SecretValue("p".into()),
    };
    let rx = RecordingMailReceiver::new();
    rx.push_batch(vec![FetchedEmail {
        uid: 1,
        email: InboundEmail {
            from_name: "A".into(),
            from_email: "a@x".into(),
            subject: "s".into(),
            body: "b".into(),
        },
    }]);
    assert_eq!(rx.fetch_new(&creds).await.unwrap().len(), 1);
    assert_eq!(rx.fetch_new(&creds).await.unwrap().len(), 0); // drained
    assert_eq!(rx.calls(), 2);
}

#[tokio::test]
async fn recording_receiver_records_marked_uids() {
    let creds = ImapCredentials {
        host: "h".into(),
        port: 993,
        username: "u".into(),
        password: SecretValue("p".into()),
    };
    let rx = RecordingMailReceiver::new();
    rx.mark_seen(&creds, &[3, 4]).await.unwrap();
    rx.mark_seen(&creds, &[5]).await.unwrap();
    assert_eq!(rx.marked(), vec![3, 4, 5]);
}

#[test]
fn tenant_mailbox_config_parses_injected_env() {
    let env = crate::app::config::MapEnv::new([
        ("OPENCOMPANY_MAIL_ADDRESS", "acme@opencompany.work"),
        ("OPENCOMPANY_MAIL_SMTP_HOST", "mail.opencompany.work"),
        ("OPENCOMPANY_MAIL_SMTP_PORT", "465"),
        ("OPENCOMPANY_MAIL_IMAP_HOST", "mail.opencompany.work"),
        ("OPENCOMPANY_MAIL_IMAP_PORT", "993"),
        ("OPENCOMPANY_MAIL_USER", "acme@opencompany.work"),
        ("OPENCOMPANY_MAIL_PASSWORD", "secret"),
    ]);
    let cfg = TenantMailboxConfig::from_env_source(&env)
        .unwrap()
        .expect("configured");
    assert_eq!(cfg.address, "acme@opencompany.work");
    assert_eq!(cfg.imap.host, "mail.opencompany.work");
    assert_eq!(cfg.imap.port, 993);
    assert_eq!(cfg.smtp.from_email, "acme@opencompany.work");
}

#[test]
fn tenant_mailbox_config_absent_is_none() {
    assert!(
        TenantMailboxConfig::from_env_source(&crate::app::config::MapEnv::default())
            .unwrap()
            .is_none()
    );
}

#[test]
fn tenant_mailbox_config_partial_is_error() {
    let env =
        crate::app::config::MapEnv::new([("OPENCOMPANY_MAIL_ADDRESS", "acme@opencompany.work")]);
    assert!(TenantMailboxConfig::from_env_source(&env).is_err());
}
