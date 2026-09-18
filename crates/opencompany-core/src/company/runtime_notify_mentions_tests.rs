use crate::company::runtime::CompanyRuntime;
use crate::ports::notifications::{Notification, NotificationStore};
use crate::ports::types::{Actor, ActorKind, CompanyId, EventSeq, Mention, MentionTarget};
use crate::ports::users::{UserRecord, UserRole, UserStatus};
use std::sync::Arc;
use tempfile::TempDir;

/// A notification store whose `append` always refuses — the store is
/// down, not merely empty.
struct FailingNotifications;

#[async_trait::async_trait]
impl NotificationStore for FailingNotifications {
    async fn append(
        &self,
        _company: &CompanyId,
        _notification: &Notification,
    ) -> crate::Result<()> {
        Err(crate::error::OpenCompanyError::Store(
            "notification append always fails in this test".to_string(),
        ))
    }
    async fn list(
        &self,
        _company: &CompanyId,
        _user: &str,
    ) -> crate::Result<Vec<crate::ports::notifications::NotificationView>> {
        Ok(Vec::new())
    }
    async fn mark_read(
        &self,
        _company: &CompanyId,
        _user: &str,
        _ids: Option<&[String]>,
    ) -> crate::Result<u64> {
        Ok(0)
    }
}

/// A one-agent company with one active human collaborator, and a
/// notification store that refuses every write.
async fn runtime_with_failing_notifications() -> (Arc<CompanyRuntime>, TempDir, String) {
    let home = tempfile::Builder::new()
        .prefix("opencompany-mention-notify-fail-")
        .tempdir()
        .expect("tempdir");
    let manifest: crate::company::CompanyManifest = toml::from_str(
        "[company]\nname = \"Acme\"\n[policy]\nmode = \"full\"\n\
         [[agent]]\nid = \"ceo\"\nrole = \"Chief\"\n",
    )
    .expect("manifest");
    let runtime = Arc::new(
        crate::runtime::RuntimeBuilder::new(home.path().to_path_buf(), manifest)
            .with_id(CompanyId::new("acme"))
            .with_notifications(Arc::new(FailingNotifications))
            .build()
            .await
            .expect("runtime"),
    );
    let user_id = crate::ports::generate_id();
    let now = crate::ports::now_millis();
    runtime
        .users()
        .upsert_user(
            runtime.id(),
            &UserRecord {
                id: user_id.clone(),
                email: "mentioned@example.test".to_string(),
                display_name: None,
                avatar: None,
                role: UserRole::Member,
                status: UserStatus::Active,
                password_hash: None,
                must_change_password: false,
                created_at_millis: now,
                last_seen_at_millis: None,
                updated_at_millis: now,
            },
        )
        .await
        .expect("seed user");
    (runtime, home, user_id)
}

/// The exact promise `notify_mentions`'s own comment makes: a
/// notification store that will not answer must not fail somebody's
/// message. Called directly rather than through the chat route, so the
/// assertion lands on the one function the promise is about.
#[tokio::test]
async fn a_failing_notification_store_does_not_panic_or_propagate() {
    let (runtime, _home, user_id) = runtime_with_failing_notifications().await;
    let mention = Mention {
        target: MentionTarget::User { id: user_id },
        text: "@mentioned".to_string(),
        offset: 0,
        quiet: false,
    };
    // The whole assertion: `notify_mentions` returns `()`, not a
    // `Result`, and this completes without panicking even though the
    // store behind it always errors.
    runtime
        .notify_mentions(
            runtime.id(),
            std::slice::from_ref(&mention),
            &EventSeq::new(1),
            Some(&Actor {
                kind: ActorKind::User,
                id: "someone-else".to_string(),
            }),
            "main",
        )
        .await;
}
