use lethetic::storage::{DatabaseClock, MonotonicInstant, WallClockInstant};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = error)]
    fn report_panic(message: &str);
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn monotonic_millis() -> f64;
    #[wasm_bindgen(js_namespace = Date, js_name = now)]
    fn epoch_millis() -> f64;
}

pub(crate) struct BrowserClock;
impl DatabaseClock for BrowserClock {
    fn current_time_monotonic(&self) -> MonotonicInstant {
        MonotonicInstant::from_nanos((monotonic_millis() * 1_000_000.0) as u128)
    }
    fn current_time_wall_clock(&self) -> WallClockInstant {
        let millis = epoch_millis();
        WallClockInstant {
            secs: (millis / 1000.0) as i64,
            micros: ((millis % 1000.0) * 1000.0) as u32,
        }
    }
}

#[wasm_bindgen(start)]
pub fn install_panic_diagnostics() {
    std::panic::set_hook(Box::new(|info| report_panic(&info.to_string())));
}
