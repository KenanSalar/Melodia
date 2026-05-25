use super::*;

#[test]
fn add_query_inserts_at_front() {
    let mut history = SearchHistory { queries: vec![] };
    add_query(&mut history, "hello");
    add_query(&mut history, "world");
    assert_eq!(history.queries, vec!["world", "hello"]);
}

#[test]
fn add_query_deduplicates() {
    let mut history = SearchHistory {
        queries: vec!["a".into(), "b".into(), "c".into()],
    };
    add_query(&mut history, "b");
    assert_eq!(history.queries, vec!["b", "a", "c"]);
}

#[test]
fn add_query_caps_at_max() {
    let mut history = SearchHistory {
        queries: (1..=10).map(|i| format!("q{i}")).collect(),
    };
    assert_eq!(history.queries.len(), 10);
    add_query(&mut history, "new");
    assert_eq!(history.queries.len(), 10);
    assert_eq!(history.queries[0], "new");
    assert_eq!(history.queries[9], "q9");
}

#[test]
fn add_query_ignores_empty() {
    let mut history = SearchHistory { queries: vec![] };
    add_query(&mut history, "");
    add_query(&mut history, "   ");
    assert!(history.queries.is_empty());
}

#[test]
fn add_query_trims_whitespace() {
    let mut history = SearchHistory { queries: vec![] };
    add_query(&mut history, "  hello  ");
    assert_eq!(history.queries, vec!["hello"]);
}
