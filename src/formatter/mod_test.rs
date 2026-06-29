use super::*;

#[test]
fn test_from_str_emoji_text() {
    assert_eq!(
        "emoji-text".parse::<OutputFormat>().unwrap(),
        OutputFormat::EmojiText
    );
}

#[test]
fn test_from_str_text_alias() {
    assert_eq!(
        "text".parse::<OutputFormat>().unwrap(),
        OutputFormat::EmojiText
    );
}

#[test]
fn test_from_str_jsonl() {
    assert_eq!(
        "jsonl".parse::<OutputFormat>().unwrap(),
        OutputFormat::Jsonl
    );
}

#[test]
fn test_from_str_json_alias() {
    assert_eq!("json".parse::<OutputFormat>().unwrap(), OutputFormat::Jsonl);
}

#[test]
fn test_from_str_invalid() {
    assert!("xml".parse::<OutputFormat>().is_err());
}

#[test]
fn test_from_str_training_json() {
    assert_eq!(
        "training-json".parse::<OutputFormat>().unwrap(),
        OutputFormat::TrainingJson
    );
}

#[test]
fn test_display_roundtrip() {
    let fmt = OutputFormat::EmojiText;
    assert_eq!(fmt.to_string().parse::<OutputFormat>().unwrap(), fmt);
    let fmt = OutputFormat::Jsonl;
    assert_eq!(fmt.to_string().parse::<OutputFormat>().unwrap(), fmt);
}
