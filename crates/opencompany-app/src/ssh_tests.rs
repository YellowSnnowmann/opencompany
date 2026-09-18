use super::*;

fn target(destination: &str) -> SshTarget {
    SshTarget {
        destination: destination.into(),
        port: None,
        remote_port: 8080,
    }
}

#[test]
fn one_target_is_one_tunnel() {
    // The roster key is what makes `open` idempotent, and the console
    // depends on that: it reopens every remembered ssh host at launch, and
    // StrictMode calls everything twice.
    assert_eq!(target("vps").key(), target("vps").key());
}

#[test]
fn a_different_port_on_the_far_side_is_a_different_tunnel() {
    let mut other = target("vps");
    other.remote_port = 9090;
    assert_ne!(target("vps").key(), other.key());
}

#[test]
fn refuses_a_destination_that_reads_as_an_option() {
    // The one shape that turns a host name into an ssh flag. There is no
    // legitimate destination like it, and this is a text field in a dialog.
    assert!(
        target("-oProxyCommand=curl evil.example")
            .problem()
            .is_some()
    );
    assert!(target("").problem().is_some());
    assert!(target("user@host with space").problem().is_some());
}

#[test]
fn accepts_what_an_operator_actually_types() {
    assert!(target("vps").problem().is_none());
    assert!(target("deploy@10.0.0.4").problem().is_none());
    assert!(target("bastion.example.com").problem().is_none());
}

#[test]
fn refuses_a_port_nothing_can_listen_on() {
    let mut zero = target("vps");
    zero.remote_port = 0;
    assert!(zero.problem().is_some());
}

#[test]
fn the_far_side_defaults_to_the_port_a_host_serves() {
    let parsed: SshTarget = serde_json::from_str(r#"{"destination":"vps"}"#).unwrap();
    assert_eq!(parsed.remote_port, DEFAULT_REMOTE_PORT);
}

#[test]
fn reads_what_the_console_sends() {
    let parsed: SshTarget =
        serde_json::from_str(r#"{"destination":"vps","port":2222,"remotePort":9090}"#).unwrap();
    assert_eq!(parsed.port, Some(2222));
    assert_eq!(parsed.remote_port, 9090);
}

#[tokio::test]
async fn keeps_no_row_for_a_tunnel_that_never_opened() {
    // A destination in the reserved `.invalid` TLD, so this fails at
    // resolution rather than after a connect timeout — and finishes at all,
    // which is what `BatchMode=yes` buys: no password prompt is waiting for
    // a window that does not exist.
    //
    // The assertion that matters is the second. A failed open must leave
    // *nothing* behind: a roster row for a tunnel that is not forwarding is
    // a host the console would probe forever and never reach.
    let mut tunnels = SshTunnels::default();
    let unreachable = target("opencompany-desktop-test.invalid");

    assert!(tunnels.open(unreachable).await.is_err());
    assert!(tunnels.list().is_empty());
}
