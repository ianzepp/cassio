use super::*;

#[test]
fn source_hash_framing_is_pinned() {
    // Every training export embeds this digest as `source_hash`; changing the
    // chunk framing (name, 0x00, body, 0xff) would rewrite all of them.
    assert_eq!(
        hash_named_chunks([("jsonl:1", "x")]),
        "sha256:8eac1007875d4270ff80c544c5f9b17ba9d6f1c9af42563ff89fd0ab4bdd1e3f"
    );
}

#[test]
fn streaming_hasher_matches_collected_chunks() {
    let lines = ["{\"a\":1}", "tail", "{\"b\":[1,2,3]}"];
    let collected = hash_named_chunks(
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| (format!("jsonl:{}", index + 1), line.to_string())),
    );

    let mut hasher = SourceHasher::new();
    for (index, line) in lines.iter().enumerate() {
        hasher.jsonl_line(index + 1, line);
    }
    assert_eq!(hasher.finish(), collected);
}

#[test]
fn jsonl_line_numbers_are_part_of_the_hash() {
    let mut first = SourceHasher::new();
    first.jsonl_line(1, "same");
    let mut second = SourceHasher::new();
    second.jsonl_line(2, "same");
    assert_ne!(first.finish(), second.finish());
}
