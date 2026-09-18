use super::*;

fn press(code: KeyCode) -> Event {
    Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn rows(ids: &[&str]) -> Vec<CompanyRow> {
    ids.iter()
        .map(|id| CompanyRow {
            id: id.to_string(),
            busy: false,
        })
        .collect()
}

fn ready(companies: Vec<CompanyRow>) -> Event {
    Event::HostReady {
        address: "http://127.0.0.1:1".into(),
        instance_id: "inst".into(),
        home: "/tmp/x".into(),
        companies,
    }
}

#[test]
fn host_ready_populates_companies_and_status() {
    let mut app = App::default();
    app.update(ready(rows(&["acme", "beta"])));
    assert!(matches!(app.host, HostStatus::Ready { .. }));
    assert_eq!(app.companies.len(), 2);
    assert_eq!(app.selected_company().map(|c| c.id.as_str()), Some("acme"));
    assert_eq!(app.log.len(), 1);
}

#[test]
fn selection_moves_within_bounds_and_survives_a_shrinking_list() {
    let mut app = App::default();
    app.update(ready(rows(&["a", "b", "c"])));
    app.update(press(KeyCode::Down));
    app.update(press(KeyCode::Char('j')));
    app.update(press(KeyCode::Down)); // past the end: clamps
    assert_eq!(app.selected, 2);
    app.update(press(KeyCode::Up));
    assert_eq!(app.selected, 1);

    app.update(Event::Companies(rows(&["a"])));
    assert_eq!(app.selected, 0);
    app.update(Event::Companies(Vec::new()));
    assert_eq!(app.selected, 0);
    assert!(app.selected_company().is_none());
    app.update(press(KeyCode::Down));
    assert_eq!(app.selected, 0);
}

#[test]
fn q_esc_and_ctrl_c_quit_but_a_release_does_not() {
    for quit in [
        press(KeyCode::Char('q')),
        press(KeyCode::Esc),
        Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
    ] {
        let mut app = App::default();
        app.update(quit);
        assert!(app.should_quit);
    }

    let mut app = App::default();
    let mut release = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
    release.kind = KeyEventKind::Release;
    app.update(Event::Key(release));
    assert!(!app.should_quit);
}

#[test]
fn host_failure_is_a_state_not_a_crash() {
    let mut app = App::default();
    app.update(Event::HostFailed("root is locked".into()));
    assert_eq!(app.host, HostStatus::Failed("root is locked".into()));
    assert!(app.log.last().unwrap().contains("root is locked"));
}

#[test]
fn log_is_bounded() {
    let mut app = App::default();
    for i in 0..(LOG_CAPACITY + 5) {
        app.update(Event::Log(format!("line {i}")));
    }
    assert_eq!(app.log.len(), LOG_CAPACITY);
    assert_eq!(app.log.first().unwrap(), "line 5");
}
