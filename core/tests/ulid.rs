use keryx_core::ulid::{Error, Ulid};

#[test]
fn text_round_trips_and_ignores_case() {
    let ulid = Ulid::new().unwrap();
    let text = ulid.to_string();
    assert_eq!(text.len(), 26);
    assert_eq!(Ulid::parse(&text).unwrap(), ulid);
    assert_eq!(Ulid::parse(&text.to_lowercase()).unwrap(), ulid);
}

#[test]
fn byte_order_is_time_order() {
    let mut early = [0u8; 16];
    let mut late = [255u8; 16];
    early[5] = 1;
    late[0] = 0;
    late[5] = 2;
    assert!(Ulid::from_bytes(early) < Ulid::from_bytes(late));
    assert_eq!(Ulid::from_bytes(early).milliseconds(), 1);
}

#[test]
fn timestamp_is_the_clock() {
    let before = Ulid::now() as u64;
    let ulid = Ulid::new().unwrap();
    let after = Ulid::now() as u64;
    assert!(ulid.milliseconds() >= before && ulid.milliseconds() <= after);
}

#[test]
fn known_text() {
    let ulid = Ulid::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
    assert_eq!(ulid.to_string(), "01ARZ3NDEKTSV4RRFFQ69G5FAV");
    assert_eq!(ulid.milliseconds(), 1469922850259);
    assert_eq!(Ulid::from_bytes(*ulid.bytes()), ulid);
}

#[test]
fn bad_text_is_refused() {
    assert!(matches!(
        Ulid::parse("short"),
        Err(Error::TextLength { .. })
    ));
    assert!(matches!(
        Ulid::parse("01ARZ3NDEKTSV4RRFFQ69G5FAI"),
        Err(Error::TextCharacter { .. })
    ));
    assert!(matches!(
        Ulid::parse("81ARZ3NDEKTSV4RRFFQ69G5FAV"),
        Err(Error::TextOverflow { .. })
    ));
}

#[test]
fn on_the_wire_a_ulid_is_its_text() {
    let ulid = Ulid::parse("01ARZ3NDEKTSV4RRFFQ69G5FAV").unwrap();
    let text = serde_json::to_string(&ulid).unwrap();
    assert_eq!(text, "\"01ARZ3NDEKTSV4RRFFQ69G5FAV\"");
    assert_eq!(serde_json::from_str::<Ulid>(&text).unwrap(), ulid);
    assert!(serde_json::from_str::<Ulid>("\"nope\"").is_err());
    assert!(serde_json::from_str::<Ulid>("5").is_err());
}
