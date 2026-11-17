//! `AutoEq`'s recommended list, which arrives off the network: a line read wrong names the wrong
//! model or builds a profile address outside the results.

use super::parse_list;
use melodia_core::entities::correction::AutoEqEntry;

fn entry(name: &str, source: &str, path: &str) -> AutoEqEntry {
    AutoEqEntry { name: name.into(), source: source.into(), path: path.into() }
}

#[test]
fn an_entry_line_reads_into_its_name_source_and_path() {
    let cases = [
        (
            "- [Sennheiser HD 650](./oratory1990/over-ear/Sennheiser%20HD%20650)",
            entry("Sennheiser HD 650", "oratory1990", "oratory1990/over-ear/Sennheiser HD 650"),
        ),
        (
            "- [Beyerdynamic DT 990 Pro (250 Ohm)](./crinacle/GRAS%2043AG-7%20over-ear/Beyerdynamic%20DT%20990%20Pro%20(250%20Ohm))",
            entry(
                "Beyerdynamic DT 990 Pro (250 Ohm)",
                "crinacle",
                "crinacle/GRAS 43AG-7 over-ear/Beyerdynamic DT 990 Pro (250 Ohm)",
            ),
        ),
        (
            "- [Etymotic ER4SR](./Rtings/Etymotic%20ER4SR)",
            entry("Etymotic ER4SR", "Rtings", "Rtings/Etymotic ER4SR"),
        ),
        (
            "- [Final Audio É5000](./oratory1990/in-ear/Final%20Audio%20%C3%895000)",
            entry("Final Audio É5000", "oratory1990", "oratory1990/in-ear/Final Audio É5000"),
        ),
        (
            "- [Sony WH-1000XM4](./oratory1990/over-ear/Sony%20WH-1000XM4)   ",
            entry("Sony WH-1000XM4", "oratory1990", "oratory1990/over-ear/Sony WH-1000XM4"),
        ),
    ];
    for (line, expected) in cases {
        assert_eq!(parse_list(line), vec![expected], "{line}");
    }
}

#[test]
fn a_link_that_would_leave_its_model_folder_is_left_out() {
    let lines = [
        ("a model with no source", "- [Model](./Model)"),
        ("a parent segment", "- [Model](./oratory1990/../../Model)"),
        ("a current segment", "- [Model](./oratory1990/./Model)"),
        ("an empty segment", "- [Model](./oratory1990//Model)"),
        ("an escaped slash", "- [Model](./oratory1990/over%2Fear/Model)"),
        ("bytes that are not UTF-8", "- [Model](./oratory1990/%FF/Model)"),
        ("a link off the list's folder", "- [Model](https://example.com/oratory1990/Model)"),
        ("no closing parenthesis", "- [Model](./oratory1990/Model"),
    ];
    for (what, line) in lines {
        assert_eq!(parse_list(line), Vec::new(), "{what}");
    }
}

/// The picker's rows index into this list, so its order is the file's.
#[test]
fn a_list_keeps_its_entries_in_order_around_what_it_cannot_read() {
    let text = concat!(
        "# Recommended headphone results\n",
        "\n",
        "- [Sennheiser HD 650](./oratory1990/over-ear/Sennheiser%20HD%20650)\n",
        "- [Model](./Model)\n",
        "- [Etymotic ER4SR](./Rtings/Etymotic%20ER4SR)\n",
    );

    let read = parse_list(text);

    let expected = vec![
        entry("Sennheiser HD 650", "oratory1990", "oratory1990/over-ear/Sennheiser HD 650"),
        entry("Etymotic ER4SR", "Rtings", "Rtings/Etymotic ER4SR"),
    ];
    assert_eq!(read, expected);
}
