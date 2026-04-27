use lethetic::loop_detector::{LoopDetector, LoopDetectorConfig, LoopDetectionMode};

#[test]
fn test_loop_detection_latest_ignore() {
    let config = LoopDetectorConfig {
        mode: LoopDetectionMode::PhraseFrequency,
        phrase_threshold: 3, // Set to 3 to catch the identified block for validation
        ..Default::default()
    };
    let detector = LoopDetector::new(config);

    // This block from the session has 3 "Wait," occurrences
    let content = r#"The path in the error is `src/../../shaders/gouraud.vs`, which resolves to `../../shaders/gouraud.vs` relative to `src/`.
Wait, if the file is at `src/main.rs`, then `../../shaders/gouraud.vs` would be at the root's sibling directory or something? Let's check the directory structure again.

Wait, the error says `src/../../shaders/gouraud.vs`.
If the file is in `./shaders/`, and we are in `./spinning_cube/src/main.rs`, then `../../shaders/gouraud.vs` should point to `../shaders/gouraud.vs` if `shaders` and `spinning_cube` are siblings.

Wait, let's check the contents of `shaders/` to see what's there."#;

    let result = detector.check(content);
    assert!(result.is_some(), "Should have detected 3 'Wait,' phrases with threshold 3");
}

#[test]
fn test_loop_detection_latest_ignore_at_default_threshold() {
    let config = LoopDetectorConfig {
        mode: LoopDetectionMode::PhraseFrequency,
        phrase_threshold: 10, // Default threshold
        ..Default::default()
    };
    let detector = LoopDetector::new(config);

    let content = r#"The path in the error is `src/../../shaders/gouraud.vs`, which resolves to `../../shaders/gouraud.vs` relative to `src/`.
Wait, if the file is at `src/main.rs`, then `../../shaders/gouraud.vs` would be at the root's sibling directory or something? Let's check the directory structure again.

Wait, the error says `src/../../shaders/gouraud.vs`.
If the file is in `./shaders/`, and we are in `./spinning_cube/src/main.rs`, then `../../shaders/gouraud.vs` should point to `../shaders/gouraud.vs` if `shaders` and `spinning_cube` are siblings.

Wait, let's check the contents of `shaders/` to see what's there."#;

    let result = detector.check(content);
    assert!(result.is_none(), "Should NOT have triggered at the default threshold of 10 (as it only has 3 phrases)");
}

#[test]
fn test_ngram_with_real_repetition() {
    let config = LoopDetectorConfig {
        mode: LoopDetectionMode::NGram,
        ngram_window: 40,
        ngram_threshold: 2,
        ..Default::default()
    };
    let detector = LoopDetector::new(config);

    let part = "Wait, if the file is at `src/main.rs`, then `../../shaders/gouraud.vs` would be at the root's sibling directory or something?";
    let content = format!("{} Some other text. {}", part, part);
    
    let result = detector.check(&content);
    assert!(result.is_some());
}
