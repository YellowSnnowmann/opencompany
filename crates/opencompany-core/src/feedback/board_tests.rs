use super::*;

#[test]
fn vote_values_round_trip_through_the_wire_integer() {
    for (value, wire) in [
        (VoteValue::Up, 1),
        (VoteValue::Down, -1),
        (VoteValue::None, 0),
    ] {
        assert_eq!(value.as_i8(), wire);
        assert_eq!(VoteValue::try_from(wire).unwrap(), value);
    }
    // Anything else is a client bug, refused before it reaches the hub.
    assert!(VoteValue::try_from(2).is_err());
    assert!(serde_json::from_str::<VoteValue>("7").is_err());
    assert_eq!(serde_json::to_string(&VoteValue::Down).unwrap(), "-1");
}

#[test]
fn tokens_parse_back_to_themselves() {
    for kind in [BoardKind::Feature, BoardKind::Bug] {
        assert_eq!(BoardKind::parse(kind.as_str()), Some(kind));
    }
    for status in [
        BoardStatus::Open,
        BoardStatus::Planned,
        BoardStatus::Completed,
        BoardStatus::Closed,
    ] {
        assert_eq!(BoardStatus::parse(status.as_str()), Some(status));
    }
    for sort in [BoardSort::Hot, BoardSort::Top, BoardSort::New] {
        assert_eq!(BoardSort::parse(sort.as_str()), Some(sort));
    }
    assert_eq!(BoardKind::parse("nonsense"), None);
    assert_eq!(BoardStatus::parse(""), None);
    assert_eq!(BoardSort::parse("HOT"), None);
}

#[test]
fn clamping_corrects_pages_the_hub_would_refuse() {
    let clamped = BoardQuery {
        page: 0,
        limit: 5_000,
        ..BoardQuery::default()
    }
    .clamped();
    assert_eq!(clamped.page, 1);
    assert_eq!(clamped.limit, MAX_LIMIT);

    let zero_limit = BoardQuery {
        limit: 0,
        ..BoardQuery::default()
    }
    .clamped();
    assert_eq!(zero_limit.limit, 1);
}
