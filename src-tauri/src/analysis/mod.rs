//! Part analyses (Analyze tab, Home › New analysis): numbers about a part where it sits on the
//! platform, plus what the view draws for them (a colour class per triangle, a marker, shapes on
//! the platform). Plain data and functions, so all of it runs without Tauri; src/jobs/analysis.rs
//! runs them for the window.

pub mod compare;
pub mod gravity;
pub mod lift;
pub mod shadow;
pub mod skin;
pub mod standard;
pub mod support;
pub mod wall;

#[cfg(test)]
mod tests;

use serde::Serialize;

/// Minimum, maximum, average and (population) standard deviation of a list of values; all zero
/// for an empty list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct Spread {
    pub min: f64,
    pub max: f64,
    pub average: f64,
    pub deviation: f64,
}

/// Builds a Spread one value at a time (Welford's method, so long lists keep their precision).
#[derive(Default)]
pub struct SpreadOf {
    n: u64,
    mean: f64,
    m2: f64,
    min: f64,
    max: f64,
}

impl SpreadOf {
    pub fn add(&mut self, v: f64) {
        if self.n == 0 {
            self.min = v;
            self.max = v;
        } else {
            self.min = self.min.min(v);
            self.max = self.max.max(v);
        }
        self.n += 1;
        let d = v - self.mean;
        self.mean += d / self.n as f64;
        self.m2 += d * (v - self.mean);
    }

    pub fn finish(&self) -> Spread {
        if self.n == 0 {
            return Spread::default();
        }
        Spread {
            min: self.min,
            max: self.max,
            average: self.mean,
            deviation: (self.m2 / self.n as f64).max(0.0).sqrt(),
        }
    }
}

/// The tilt of a triangle against the X/Y plane in degrees (0 flat, 90 upright) from its unit
/// normal's Z.
pub fn tilt_degrees(nz: f64) -> f64 {
    nz.abs().clamp(0.0, 1.0).acos().to_degrees()
}
