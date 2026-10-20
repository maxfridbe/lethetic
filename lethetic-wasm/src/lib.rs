//! JavaScript bindings for the CLI library. Agent logic belongs in `lethetic`.
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct BrowserRuntime(lethetic::engine::Conversation);
#[wasm_bindgen]
impl BrowserRuntime {
    #[wasm_bindgen(constructor)]
    pub fn new(system: &str, tools: &str) -> Result<Self, String> {
        lethetic::engine::Conversation::new(system, tools).map(Self)
    }
    pub fn set_limits(&mut self, context: usize, output: u32) -> Result<(), String> {
        self.0.set_limits(context, output)
    }
    pub fn begin(&mut self, prompt: &str) -> Result<(), String> {
        self.0.begin(prompt)
    }
    pub fn request(&mut self, provider: &str, model: &str) -> Result<String, String> {
        self.0.request(provider, model)
    }
    pub fn last_request_tokens(&self) -> usize {
        self.0.last_request_tokens()
    }
    pub fn accept(&mut self, provider: &str, response: &str) -> Result<String, String> {
        self.0.accept(provider, response)
    }
    pub fn tool_result(&mut self, id: &str, output: &str) -> Result<(), String> {
        self.0.tool_result(id, output)
    }
    pub fn rollback(&mut self) {
        self.0.rollback();
    }
    pub fn clear(&mut self) {
        self.0.clear();
    }
    pub fn history(&self) -> String {
        self.0.history()
    }
    pub fn restore(&mut self, history: &str) -> Result<(), String> {
        self.0.restore(history)
    }
}

#[wasm_bindgen]
pub struct BrowserStream(lethetic::engine::ProviderStream);
#[wasm_bindgen]
impl BrowserStream {
    #[wasm_bindgen(constructor)]
    pub fn new(provider: &str) -> Result<Self, String> {
        lethetic::engine::ProviderStream::new(provider).map(Self)
    }
    pub fn push(&mut self, data: &str) -> Result<String, String> {
        self.0.push(data)
    }
    pub fn finish(&mut self) -> Result<String, String> {
        self.0.finish()
    }
}

#[wasm_bindgen]
pub fn retryable_error(error: &str) -> bool {
    lethetic::engine::retryable_error(error)
}
#[wasm_bindgen]
pub fn retry_delay_ms(attempt: u32) -> u32 {
    lethetic::engine::retry_delay_ms(attempt)
}
#[wasm_bindgen]
pub fn check_loop(text: &str, mode: &str) -> Result<Option<String>, String> {
    use lethetic::loop_detector::{LoopDetector, LoopDetectorConfig};
    let mode = serde_json::from_value(serde_json::json!(mode)).map_err(|e| e.to_string())?;
    let detector = LoopDetector::new(LoopDetectorConfig {
        mode,
        ..Default::default()
    });
    Ok(detector.check(text).map(|detection| detection.reason))
}
#[wasm_bindgen]
pub fn calculate(expression: &str) -> Result<f64, String> {
    lethetic::calculator::evaluate(expression)
}

#[wasm_bindgen]
pub struct BrowserCompaction(lethetic::engine::compaction::Compaction);
#[wasm_bindgen]
impl BrowserCompaction {
    #[wasm_bindgen(constructor)]
    pub fn new(history: &str, input_tokens: usize, output_tokens: u32) -> Result<Self, String> {
        let source = lethetic::engine::compaction::browser_transcript(history)?;
        lethetic::engine::compaction::Compaction::new(&source, input_tokens, output_tokens)
            .map(Self)
    }
    pub fn requests(&self) -> Result<String, String> {
        serde_json::to_string(&self.0.requests()).map_err(|error| error.to_string())
    }
    pub fn accept(&mut self, id: usize, text: &str) -> Result<(), String> {
        self.0.accept(id, text)
    }
    pub fn progress(&self) -> Result<String, String> {
        serde_json::to_string(&self.0.progress()).map_err(|error| error.to_string())
    }
    pub fn result(&self) -> Option<String> {
        self.0.result().map(str::to_owned)
    }
}
