//! Where a model's profile is fetched from.

use super::profile_url;
use melodia_core::entities::correction::AutoEqEntry;

fn url_for(path: &str) -> Option<String> {
    let entry = AutoEqEntry { name: "".into(), source: "".into(), path: path.into() };
    profile_url(&entry).ok().map(String::from)
}

/// A `#` or `?` joined in as text would cut the address at a fragment or a query, and a `%` would
/// read as an escape.
#[test]
fn a_profile_is_fetched_from_its_models_folder_with_each_segment_escaped() {
    let cases = [
        (
            "oratory1990/over-ear/Sennheiser HD 650",
            "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/oratory1990/over-ear/Sennheiser%20HD%20650/Sennheiser%20HD%20650%20ParametricEQ.txt",
        ),
        (
            "crinacle/GRAS 43AG-7 over-ear/Beyerdynamic DT 990 Pro (250 Ohm)",
            "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/crinacle/GRAS%2043AG-7%20over-ear/Beyerdynamic%20DT%20990%20Pro%20(250%20Ohm)/Beyerdynamic%20DT%20990%20Pro%20(250%20Ohm)%20ParametricEQ.txt",
        ),
        (
            "oratory1990/over-ear/Model #2?",
            "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/oratory1990/over-ear/Model%20%232%3F/Model%20%232%3F%20ParametricEQ.txt",
        ),
        (
            "Rtings/100% Bass",
            "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/Rtings/100%25%20Bass/100%25%20Bass%20ParametricEQ.txt",
        ),
    ];
    for (path, expected) in cases {
        assert_eq!(url_for(path).as_deref(), Some(expected), "{path}");
    }
}
