use super::*;

fn tmp_bundle() -> (tempfile::TempDir, Bundle) {
    let dir = tempfile::tempdir().expect("tempdir");
    let bundle = Bundle::new(dir.path().to_path_buf(), &CompanyId::new("acme"));
    (dir, bundle)
}

#[tokio::test]
async fn keygen_persists_and_reloads_same_agent_id() {
    let (_dir, bundle) = tmp_bundle();
    let first = load_or_create_signer(&bundle).await.expect("create");
    assert!(bundle.agent_key().exists(), "seed file written");

    let again = load_or_create_signer(&bundle).await.expect("reload");
    assert_eq!(
        first.agent_id(),
        again.agent_id(),
        "reload yields identical identity"
    );
}

#[test]
fn agent_id_is_base58_of_32_byte_pubkey() {
    let signer = LocalSigner::generate();
    let id = signer.agent_id();
    let decoded = bs58::decode(&id).into_vec().expect("base58");
    assert_eq!(decoded.len(), 32);
    assert_eq!(decoded, signer.public_key_bytes().to_vec());
}

#[cfg(unix)]
#[tokio::test]
async fn seed_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let (_dir, bundle) = tmp_bundle();
    load_or_create_signer(&bundle).await.expect("create");
    let meta = std::fs::metadata(bundle.agent_key()).expect("metadata");
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
}

#[test]
fn sign_verify_round_trip_and_wrong_key_fails() {
    let signer = LocalSigner::generate();
    let other = LocalSigner::generate();
    let msg = b"canonical-payload";

    let sig = signer.sign_b58(msg);
    verify_b58(&signer.agent_id(), msg, &sig).expect("valid signature verifies");

    // Same signature attributed to a different agent must fail.
    assert!(verify_b58(&other.agent_id(), msg, &sig).is_err());
    // Tampered message must fail.
    assert!(verify_b58(&signer.agent_id(), b"tampered", &sig).is_err());
}

#[test]
fn hex_codec_round_trips() {
    let seed = [
        0u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 30, 255,
    ];
    let hex = encode_hex(&seed);
    assert_eq!(hex.len(), 64);
    assert_eq!(decode_hex_seed(&hex), Some(seed));
    assert_eq!(decode_hex_seed("xyz"), None);
}
