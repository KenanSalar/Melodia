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
