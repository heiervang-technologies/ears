use ears::text_filters::TextFilters;

#[test]
fn test_strict_alphabet() {
    let mut filters = TextFilters::new();
    filters.strict_alphabet = true;

    // Test that English keeps Latin script
    assert_eq!(filters.apply("Hello world", Some("en")), "Hello world");

    // Test that English drops Thai
    assert_eq!(
        filters.apply("ขอเชิญ ขอเชิญ ฮอร์It finally works", Some("en")),
        ""
    );

    // Test that English drops Chinese
    assert_eq!(filters.apply("下车搵架", Some("en")), "");

    // Test that Chinese keeps Chinese (since Chinese is not in the strict check list yet)
    assert_eq!(filters.apply("下车搵架", Some("zh")), "下车搵架");
}
