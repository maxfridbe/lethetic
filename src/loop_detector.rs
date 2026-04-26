use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoopDetectionMode {
    Off,
    BlockLimit,
    NGram,
    PhraseFrequency,
    Combined,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopDetectorConfig {
    pub mode: LoopDetectionMode,
    pub block_limit: usize,
    pub ngram_window: usize,
    pub ngram_threshold: usize,
    pub phrase_threshold: usize,
}

impl Default for LoopDetectorConfig {
    fn default() -> Self {
        Self {
            mode: LoopDetectionMode::Combined,
            block_limit: 5000,
            ngram_window: 64,
            ngram_threshold: 3,
            phrase_threshold: 4,
        }
    }
}

pub struct LoopDetector {
    pub config: LoopDetectorConfig,
}

impl LoopDetector {
    pub fn new(config: LoopDetectorConfig) -> Self {
        Self { config }
    }

    /// Checks for loops in the given full block content.
    /// Returns Some(reason) if a loop is detected.
    pub fn check(&self, content: &str) -> Option<String> {
        if self.config.mode == LoopDetectionMode::Off {
            return None;
        }

        // 1. Block Limit Check
        if (self.config.mode == LoopDetectionMode::BlockLimit || self.config.mode == LoopDetectionMode::Combined) 
            && content.len() > self.config.block_limit {
            return Some(format!("Block length ({} chars) exceeded limit", content.len()));
        }

        // 2. Phrase Frequency Check
        if self.config.mode == LoopDetectionMode::PhraseFrequency || self.config.mode == LoopDetectionMode::Combined {
            let phrases = ["Actually,", "Wait,", "I'll just", "I will just", "Instead, I'll", "Trying again", "Wait I'll try"];
            let mut total_phrases = 0;
            for p in phrases {
                total_phrases += content.matches(p).count();
            }
            if total_phrases >= self.config.phrase_threshold {
                return Some(format!("Excessive self-correction detected ({} phrases)", total_phrases));
            }
        }

        // 3. N-Gram Repetition Check
        if self.config.mode == LoopDetectionMode::NGram || self.config.mode == LoopDetectionMode::Combined {
            if content.len() >= self.config.ngram_window * 2 {
                let window_size = self.config.ngram_window;
                let last_window = &content[content.len() - window_size..];
                
                // Count how many times this specific window appears in the whole block
                let occurrences = content.matches(last_window).count();
                if occurrences >= self.config.ngram_threshold {
                    return Some(format!("Repeating pattern detected (sequence seen {} times)", occurrences));
                }
            }
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_limit() {
        let config = LoopDetectorConfig {
            mode: LoopDetectionMode::BlockLimit,
            block_limit: 10,
            ..Default::default()
        };
        let detector = LoopDetector::new(config);
        assert!(detector.check("short").is_none());
        assert!(detector.check("this is a very long block").is_some());
    }

    #[test]
    fn test_ngram_detection() {
        let config = LoopDetectorConfig {
            mode: LoopDetectionMode::NGram,
            ngram_window: 5,
            ngram_threshold: 3,
            ..Default::default()
        };
        let detector = LoopDetector::new(config);
        // "abcde" repeated 3 times
        assert!(detector.check("abcde...abcde...abcde").is_some());
        assert!(detector.check("abcdefghijk").is_none());
    }

    #[test]
    fn test_phrase_frequency() {
        let config = LoopDetectorConfig {
            mode: LoopDetectionMode::PhraseFrequency,
            phrase_threshold: 3,
            ..Default::default()
        };
        let detector = LoopDetector::new(config);
        assert!(detector.check("Actually, I'll do this. Wait, Actually, no. Actually, yes.").is_some());
        assert!(detector.check("I will do this normally.").is_none());
    }
}
