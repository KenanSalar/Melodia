use super::super::device_map::DeviceMap;

const CARD: &str = "hw:CARD=UMC22";

/// System default is a field of its own, so clearing a device's value can't take it, and the
/// reverse.
#[test]
fn system_default_and_a_device_keep_their_own_value() {
    let mut map = DeviceMap::<i32>::default();
    map.insert(None, -2);
    map.insert(Some(CARD.to_owned()), 4);

    let removed = map.remove(Some(CARD));

    assert_eq!(removed, Some(4), "the device's own value comes back");
    assert_eq!(map.for_device(Some(CARD)), None);
    assert_eq!(map.for_device(None), Some(&-2), "System default's stays");
}

fn holding(device: Option<&str>, value: i32) -> DeviceMap<i32> {
    let mut map = DeviceMap::default();
    map.insert(device.map(str::to_owned), value);
    map
}

/// A write changing one field has to reach what the device already holds: a fresh default would
/// drop every field the write didn't name.
#[test]
fn an_edit_reaches_what_the_device_already_holds() {
    let cases = [
        ("System default, held", holding(None, 4), None, 5),
        ("System default, absent", DeviceMap::default(), None, 1),
        ("a card, held", holding(Some(CARD), 4), Some(CARD), 5),
        ("a card, absent", DeviceMap::default(), Some(CARD), 1),
    ];
    for (what, mut map, device, expected) in cases {
        *map.get_or_insert_default(device.map(str::to_owned)) += 1;

        assert_eq!(map.for_device(device), Some(&expected), "{what}");
    }
}
