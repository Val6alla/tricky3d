//! Import progress for the status bar, throttled so a big file sends a few dozen messages, not
//! millions. Free of Tauri types: the command layer passes a closure that writes to its channel.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Read,
    Parse,
    Weld,
    Analyze,
}

impl Stage {
    /// The `stage` string in `ImportProgress` (src/io/ipc.ts).
    pub fn name(self) -> &'static str {
        match self {
            Stage::Read => "read",
            Stage::Parse => "parse",
            Stage::Weld => "weld",
            Stage::Analyze => "analyze",
        }
    }

    /// The share of the whole import each stage covers, as (start, end).
    fn span(self) -> (f64, f64) {
        match self {
            Stage::Read => (0.0, 0.15),
            Stage::Parse => (0.15, 0.65),
            Stage::Weld => (0.65, 0.8),
            Stage::Analyze => (0.8, 1.0),
        }
    }
}

/// Smallest change of the overall fraction worth a message.
const STEP: f64 = 0.025;

pub struct Progress<'a> {
    emit: Option<Box<dyn FnMut(Stage, f64) + 'a>>,
    stage: Option<Stage>,
    sent: f64,
}

impl<'a> Progress<'a> {
    /// `emit(stage, fraction)` receives the overall fraction (0..1, never decreasing).
    pub fn new(emit: impl FnMut(Stage, f64) + 'a) -> Progress<'a> {
        Progress {
            emit: Some(Box::new(emit)),
            stage: None,
            sent: 0.0,
        }
    }

    /// For tests and callers nobody watches.
    pub fn silent() -> Progress<'static> {
        Progress {
            emit: None,
            stage: None,
            sent: 0.0,
        }
    }

    /// `local` is how far along `stage` is (0..1).
    pub fn report(&mut self, stage: Stage, local: f64) {
        let (start, end) = stage.span();
        let fraction = (start + (end - start) * local.clamp(0.0, 1.0)).max(self.sent);
        if self.stage == Some(stage)
            && fraction - self.sent < STEP
            && !(local >= 1.0 && fraction > self.sent)
        {
            return;
        }
        self.stage = Some(stage);
        self.sent = fraction;
        if let Some(emit) = self.emit.as_mut() {
            emit(stage, fraction);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttles_and_never_goes_back() {
        let mut seen = Vec::new();
        {
            let mut p = Progress::new(|s, f| seen.push((s, f)));
            for i in 0..=100_000 {
                p.report(Stage::Parse, i as f64 / 100_000.0);
            }
            p.report(Stage::Read, 0.5);
            p.report(Stage::Analyze, 1.0);
        }
        assert!(seen.len() < 40, "{} messages", seen.len());
        assert!(seen.windows(2).all(|w| w[1].1 >= w[0].1));
        assert_eq!(seen.last(), Some(&(Stage::Analyze, 1.0)));
    }
}
