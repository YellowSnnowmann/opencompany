//! Default-choice tests: the new JSON default shape, the lenient
//! reader, and model collapsing (split out of `store_tests.rs`).

use super::store_tests_support::*;
use super::*;

// ---- the default's new shape (keys rework, issue #2306, slice 2b) ------

#[tokio::test]
async fn a_json_default_reads_provider_and_model() {
    let secrets = MemSecrets::default();
    secrets
        .set(
            &company(),
            DEFAULT_PROVIDER_KEY,
            SecretValue("  {\"provider\":\" acme \",\"model\":\" acme/other-model \"}\n".into()),
        )
        .await
        .unwrap();
    assert_eq!(
        load_default(&company(), &secrets).await.unwrap(),
        DefaultChoice::Full(ModelChoice {
            provider: "acme".to_string(),
            model: "acme/other-model".to_string(),
        })
    );
}

#[tokio::test]
async fn a_bare_slug_default_reads_as_provider_without_model() {
    let secrets = MemSecrets::default();
    secrets
        .set(
            &company(),
            DEFAULT_PROVIDER_KEY,
            SecretValue(" acme\n".into()),
        )
        .await
        .unwrap();
    assert_eq!(
        load_default(&company(), &secrets).await.unwrap(),
        DefaultChoice::ProviderOnly("acme".to_string())
    );
    assert_eq!(
        load_default_slug(&company(), &secrets)
            .await
            .unwrap()
            .as_deref(),
        Some("acme")
    );
}

#[tokio::test]
async fn a_blank_model_in_json_reads_as_provider_only() {
    let secrets = MemSecrets::default();
    for raw in [
        r#"{"provider":"acme"}"#,
        r#"{"provider":"acme","model":null}"#,
        r#"{"provider":"acme","model":"  "}"#,
    ] {
        secrets
            .set(
                &company(),
                DEFAULT_PROVIDER_KEY,
                SecretValue(raw.to_string()),
            )
            .await
            .unwrap();
        assert_eq!(
            load_default(&company(), &secrets).await.unwrap(),
            DefaultChoice::ProviderOnly("acme".to_string()),
            "raw: {raw}"
        );
    }
}

#[tokio::test]
async fn an_unparseable_json_default_is_an_error() {
    let secrets = MemSecrets::default();
    for raw in [
        r#"{"provider":"#,
        r#"{"model":"x"}"#,
        r#"{"provider":"  ","model":"x"}"#,
        r#"{"provider":5}"#,
    ] {
        secrets
            .set(
                &company(),
                DEFAULT_PROVIDER_KEY,
                SecretValue(raw.to_string()),
            )
            .await
            .unwrap();
        let err = load_default(&company(), &secrets).await.unwrap_err();
        assert!(
            matches!(err, OpenCompanyError::Store(_)),
            "raw: {raw}: {err}"
        );
        assert!(
            err.to_string().contains("inference default"),
            "raw: {raw}: {err}"
        );
    }
}

/// Round-3a review P2-4: the read side of a corrupt default must degrade,
/// never 500 — see [`load_default_lenient`]'s own doc for why.
#[tokio::test]
async fn a_corrupt_default_reads_as_unset_and_unreadable_never_written() {
    let secrets = MemSecrets::default();
    secrets
        .set(
            &company(),
            DEFAULT_PROVIDER_KEY,
            SecretValue(r#"{oops"#.to_string()),
        )
        .await
        .unwrap();

    let (choice, unreadable) = load_default_lenient(&company(), &secrets).await;
    assert_eq!(choice, DefaultChoice::Unset);
    assert!(unreadable);

    // Never rewritten: the corrupt value is still on disk, byte for byte,
    // so fixing it by hand and reading again recovers on its own.
    let raw = secrets
        .get(&company(), DEFAULT_PROVIDER_KEY)
        .await
        .unwrap()
        .map(|SecretValue(v)| v);
    assert_eq!(raw.as_deref(), Some(r#"{oops"#));
}

#[tokio::test]
async fn a_readable_default_round_trips_through_the_lenient_reader_unmarked() {
    let secrets = MemSecrets::default();
    let choice = ModelChoice {
        provider: "acme".to_string(),
        model: "test-model".to_string(),
    };
    set_default_choice(&company(), &secrets, &choice)
        .await
        .unwrap();

    let (read, unreadable) = load_default_lenient(&company(), &secrets).await;
    assert_eq!(read, DefaultChoice::Full(choice));
    assert!(!unreadable);
}

#[tokio::test]
async fn a_cleared_default_reads_unset() {
    let secrets = MemSecrets::default();
    assert_eq!(
        load_default(&company(), &secrets).await.unwrap(),
        DefaultChoice::Unset
    );
    assert_eq!(load_default_slug(&company(), &secrets).await.unwrap(), None);

    clear_default_slug(&company(), &secrets).await.unwrap();
    assert_eq!(
        load_default(&company(), &secrets).await.unwrap(),
        DefaultChoice::Unset
    );

    secrets
        .set(&company(), DEFAULT_PROVIDER_KEY, SecretValue("   ".into()))
        .await
        .unwrap();
    assert_eq!(
        load_default(&company(), &secrets).await.unwrap(),
        DefaultChoice::Unset
    );
    assert_eq!(load_default_slug(&company(), &secrets).await.unwrap(), None);
}

#[tokio::test]
async fn setting_a_default_choice_is_one_json_write() {
    let secrets = MemSecrets::default();
    set_default_choice(
        &company(),
        &secrets,
        &ModelChoice {
            provider: " tinyhumans ".to_string(),
            model: " acme/test-model ".to_string(),
        },
    )
    .await
    .unwrap();
    // Scoped so the guard is released before the `.await` below: clippy's
    // `await_holding_lock` is right that a `std` guard across an await is a
    // deadlock waiting to happen, even though this one never contends.
    {
        let map = secrets.map.lock().unwrap();
        assert_eq!(map.len(), 1, "one write: {map:?}");
        assert_eq!(
            map.get(DEFAULT_PROVIDER_KEY).map(String::as_str),
            Some(r#"{"provider":"tinyhumans","model":"acme/test-model"}"#)
        );
    }
    assert_eq!(
        load_default(&company(), &secrets).await.unwrap(),
        DefaultChoice::Full(ModelChoice {
            provider: "tinyhumans".to_string(),
            model: "acme/test-model".to_string(),
        })
    );
}

#[tokio::test]
async fn a_default_choice_without_a_model_is_refused_before_writing() {
    let secrets = MemSecrets::default();
    let err = set_default_choice(
        &company(),
        &secrets,
        &ModelChoice {
            provider: "acme".to_string(),
            model: "  ".to_string(),
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, OpenCompanyError::InvalidRequest(_)), "{err}");
    assert!(secrets.map.lock().unwrap().is_empty());
}

#[tokio::test]
async fn load_default_slug_reads_the_provider_out_of_a_json_default() {
    let secrets = MemSecrets::default();
    secrets
        .set(
            &company(),
            DEFAULT_PROVIDER_KEY,
            SecretValue(r#"{"provider":"acme","model":"acme/other-model"}"#.to_string()),
        )
        .await
        .unwrap();
    assert_eq!(
        load_default_slug(&company(), &secrets)
            .await
            .unwrap()
            .as_deref(),
        Some("acme")
    );
}

#[tokio::test]
async fn a_row_with_one_distinct_model_collapses_to_it() {
    let secrets = MemSecrets::default();
    let mut d = draft("acme");
    d.models = crate::company::INFERENCE_TIERS
        .iter()
        .map(|t| ((*t).to_string(), "acme/other-model".to_string()))
        .collect();
    d.models
        .insert("chat-v1".to_string(), " acme/other-model ".to_string());
    d.models.insert("extra".to_string(), "".to_string());
    put_provider(&company(), &secrets, d).await.unwrap();
    let row = get_provider(&company(), &secrets, "acme")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.model(), ModelOnRow::One("acme/other-model".to_string()));
}

#[test]
fn a_row_with_no_model_reads_none() {
    assert_eq!(model_on_row(&BTreeMap::new()), ModelOnRow::None);
    let mut blank = BTreeMap::new();
    blank.insert("chat-v1".to_string(), "  ".to_string());
    assert_eq!(model_on_row(&blank), ModelOnRow::None);
}

#[test]
fn a_row_with_two_distinct_models_is_ambiguous_never_picked() {
    let mut models = BTreeMap::new();
    models.insert("chat-v1".to_string(), "b-model".to_string());
    models.insert("agentic-v1".to_string(), "a-model".to_string());
    models.insert("reasoning-v1".to_string(), "a-model".to_string());
    assert_eq!(
        model_on_row(&models),
        ModelOnRow::Ambiguous(vec!["a-model".to_string(), "b-model".to_string()])
    );
}

#[tokio::test]
async fn entry_zero_models_collapse_the_same_way() {
    let secrets = MemSecrets::default();
    let uniform = crate::company::INFERENCE_TIERS
        .iter()
        .map(|t| ((*t).to_string(), "acme/test-model".to_string()))
        .collect();
    super::super::save_runtime_config(
        &company(),
        &secrets,
        &RuntimeInference {
            provider: "openrouter".to_string(),
            base_url: None,
            models: uniform,
        },
    )
    .await
    .unwrap();
    let zero = entry_zero(&company(), &secrets).await.unwrap().unwrap();
    assert_eq!(zero.model(), ModelOnRow::One("acme/test-model".to_string()));

    let mut ambiguous = BTreeMap::new();
    ambiguous.insert("chat-v1".to_string(), "x/a".to_string());
    ambiguous.insert("agentic-v1".to_string(), "x/b".to_string());
    super::super::save_runtime_config(
        &company(),
        &secrets,
        &RuntimeInference {
            provider: "openrouter".to_string(),
            base_url: None,
            models: ambiguous,
        },
    )
    .await
    .unwrap();
    let zero = entry_zero(&company(), &secrets).await.unwrap().unwrap();
    assert!(matches!(zero.model(), ModelOnRow::Ambiguous(_)));
}
