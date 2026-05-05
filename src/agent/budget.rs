use std::time::Duration;
use web_time::Instant;

#[derive(Clone, Debug)]
pub struct Budget {
    pub max_tokens: Option<u64>,
    pub max_seconds: Option<u64>,
    pub max_tool_calls: Option<u32>,
    pub max_steps: Option<u32>,
    used_tokens: u64,
    used_tool_calls: u32,
    used_steps: u32,
    started_at: Instant,
}

impl Budget {
    pub fn new(
        max_tokens: Option<u64>,
        max_seconds: Option<u64>,
        max_tool_calls: Option<u32>,
        max_steps: Option<u32>,
    ) -> Self {
        Self {
            max_tokens,
            max_seconds,
            max_tool_calls,
            max_steps,
            used_tokens: 0,
            used_tool_calls: 0,
            used_steps: 0,
            started_at: Instant::now(),
        }
    }

    pub fn unlimited() -> Self {
        Self::new(None, None, None, None)
    }

    pub fn record_tokens(&mut self, n: u64) {
        self.used_tokens = self.used_tokens.saturating_add(n);
    }

    pub fn record_tool_call(&mut self) {
        self.used_tool_calls = self.used_tool_calls.saturating_add(1);
    }

    pub fn record_step(&mut self) {
        self.used_steps = self.used_steps.saturating_add(1);
    }

    pub fn used_tokens(&self) -> u64 {
        self.used_tokens
    }

    pub fn used_tool_calls(&self) -> u32 {
        self.used_tool_calls
    }

    pub fn used_steps(&self) -> u32 {
        self.used_steps
    }

    pub fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }

    pub fn is_exceeded(&self) -> Option<&'static str> {
        if let Some(max) = self.max_tokens {
            if self.used_tokens >= max {
                return Some("max_tokens");
            }
        }
        if let Some(max) = self.max_seconds {
            if self.started_at.elapsed().as_secs() >= max {
                return Some("max_seconds");
            }
        }
        if let Some(max) = self.max_tool_calls {
            if self.used_tool_calls >= max {
                return Some("max_tool_calls");
            }
        }
        if let Some(max) = self.max_steps {
            if self.used_steps >= max {
                return Some("max_steps");
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unlimited_never_exceeds() {
        let mut b = Budget::unlimited();
        b.record_tokens(1_000_000);
        b.record_tool_call();
        b.record_step();
        assert!(b.is_exceeded().is_none());
    }

    #[test]
    fn token_budget_exhaustion() {
        let mut b = Budget::new(Some(100), None, None, None);
        b.record_tokens(50);
        assert!(b.is_exceeded().is_none());
        b.record_tokens(50);
        assert_eq!(b.is_exceeded(), Some("max_tokens"));
    }

    #[test]
    fn tool_call_budget_exhaustion() {
        let mut b = Budget::new(None, None, Some(2), None);
        b.record_tool_call();
        assert!(b.is_exceeded().is_none());
        b.record_tool_call();
        assert_eq!(b.is_exceeded(), Some("max_tool_calls"));
    }

    #[test]
    fn step_budget_exhaustion() {
        let mut b = Budget::new(None, None, None, Some(1));
        b.record_step();
        assert_eq!(b.is_exceeded(), Some("max_steps"));
    }

    #[test]
    fn zero_seconds_immediately_exceeded() {
        let b = Budget::new(None, Some(0), None, None);
        assert_eq!(b.is_exceeded(), Some("max_seconds"));
    }

    #[test]
    fn token_overflow_saturates() {
        let mut b = Budget::new(None, None, None, None);
        b.record_tokens(u64::MAX);
        b.record_tokens(1);
        assert_eq!(b.used_tokens(), u64::MAX);
    }
}
