//! Microphone conditioning adapted from sttme 6928497 (MIT; see docs/licenses/STTME-MIT.txt).
use std::{
    f32::consts::PI,
    sync::atomic::{AtomicU32, Ordering},
};

/// Remove DC bias/low rumble; mute the first 40 ms and fade in over the next 25 ms.
pub(crate) struct Conditioner {
    alpha: f32,
    previous: Option<f32>,
    output: f32,
    index: usize,
    mute: usize,
    ramp: usize,
}
impl Conditioner {
    pub fn new(rate: u32) -> Self {
        Self {
            alpha: 1.0 / (1.0 + 2.0 * PI * 85.0 / rate.max(1) as f32),
            previous: None,
            output: 0.0,
            index: 0,
            mute: rate as usize * 40 / 1000,
            ramp: (rate as usize * 25).div_ceil(1000).max(1),
        }
    }
    pub fn process(&mut self, raw: f32) -> f32 {
        let raw = if raw.is_finite() {
            raw.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let output = self.alpha * (self.output + raw - self.previous.unwrap_or(raw));
        self.previous = Some(raw);
        self.output = output;
        let gain = if self.index < self.mute {
            0.0
        } else if self.index < self.mute + self.ramp {
            0.5 * (1.0 - (PI * (self.index - self.mute) as f32 / self.ramp as f32).cos())
        } else {
            1.0
        };
        self.index = self.index.saturating_add(1);
        output * gain
    }
}

/// Hold the largest fresh envelope until the next UI poll; distinguish silence from no packet.
pub(crate) struct Meter(AtomicU32);
impl Default for Meter {
    fn default() -> Self {
        Self(AtomicU32::new(u32::MAX))
    }
}
impl Meter {
    pub fn record(&self, level: f32) {
        if !level.is_finite() || level < 0.0 {
            return;
        }
        let _ = self
            .0
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |previous| {
                Some(
                    if previous == u32::MAX {
                        level
                    } else {
                        f32::from_bits(previous).max(level)
                    }
                    .to_bits(),
                )
            });
    }
    pub fn take(&self) -> Option<f32> {
        let bits = self.0.swap(u32::MAX, Ordering::Relaxed);
        (bits != u32::MAX).then(|| f32::from_bits(bits))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bias_and_wakeup_pop_are_removed_while_voice_survives() {
        for rate in [16000, 48000] {
            let mut conditioner = Conditioner::new(rate);
            for i in 0..rate / 4 {
                let output = conditioner.process(if i < rate / 50 { -0.42 } else { 0.115 });
                if i < rate * 40 / 1000 {
                    assert_eq!(output, 0.0);
                }
                if i > rate / 5 {
                    assert!(output.abs() < 0.001);
                }
            }
            let mut peak = 0.0_f32;
            for i in 0..rate / 10 {
                let voice = 0.08 * (2.0 * PI * 250.0 * i as f32 / rate as f32).sin();
                peak = peak.max(conditioner.process(0.115 + voice).abs());
            }
            assert!(peak > 0.06);
            assert!(conditioner.process(f32::NAN).is_finite());
        }
    }
    #[test]
    fn meter_keeps_brief_peaks_and_distinguishes_no_packet_from_silence() {
        let meter = Meter::default();
        assert_eq!(meter.take(), None);
        meter.record(0.8);
        meter.record(0.1);
        meter.record(f32::NAN);
        assert_eq!(meter.take(), Some(0.8));
        assert_eq!(meter.take(), None);
        meter.record(0.0);
        assert_eq!(meter.take(), Some(0.0));
    }
}
