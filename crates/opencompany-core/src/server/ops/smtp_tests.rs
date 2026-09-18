use super::*;

#[test]
fn status_drops_password() {
    let creds = SmtpCredentials {
        host: "smtp.example.com".into(),
        port: 587,
        security: SmtpSecurity::Starttls,
        username: "user".into(),
        password: SecretValue("s3cret-pw".into()),
        from_name: "Acme".into(),
        from_email: "ceo@acme.test".into(),
    };
    let status = SmtpStatus::from_credentials(&creds);
    let json = serde_json::to_string(&status).unwrap();
    assert!(!json.contains("s3cret-pw"), "password leaked into status");
    assert!(json.contains("smtp.example.com"));
    assert!(status.configured);
}

#[test]
fn local_part_splits_address() {
    assert_eq!(local_part("ceo@acme.test"), "ceo");
    assert_eq!(local_part("bare"), "bare");
}

#[tokio::test]
async fn recording_sender_captures_send() {
    use crate::server::ops::mailer::{MailSender, RecordingMailSender};

    let sender = RecordingMailSender::new();
    let creds = MailCredentials::Smtp(SmtpCredentials {
        host: "h".into(),
        port: 25,
        security: SmtpSecurity::None,
        username: "u".into(),
        password: SecretValue("p".into()),
        from_name: String::new(),
        from_email: "from@x.test".into(),
    });
    let email = OutboundEmail {
        to: "to@x.test".into(),
        subject: "s".into(),
        body: "b".into(),
    };
    sender.send(&creds, &email).await.unwrap();
    assert_eq!(sender.sent().len(), 1);
    assert_eq!(sender.sent()[0].0, "from@x.test");
}
