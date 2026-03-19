/// Variable difficulty algorithm for stratum clients.
/// Retargets after N shares, targeting T seconds between shares.
/// Clamps adjustment to 0.25x-4x, skips if within 0.85-1.15x.

use std::time::Instant;

/// Configuration for vardiff
#[derive(Debug, Clone)]
pub struct VardiffConfig {
    /// Target time between shares in seconds
    pub target_time: f64,
    /// Number of shares before retargeting
    pub retarget_shares: u32,
    /// Minimum difficulty
    pub min_difficulty: f64,
    /// Maximum difficulty
    pub max_difficulty: f64,
}

impl Default for VardiffConfig {
    fn default() -> Self {
        Self {
            target_time: 10.0,
            retarget_shares: 12,
            min_difficulty: 0.001,
            max_difficulty: 2_000_000_000.0,
        }
    }
}

/// Per-client vardiff state
#[derive(Debug, Clone)]
pub struct VardiffState {
    /// Current difficulty
    pub difficulty: f64,
    /// Number of shares since last retarget
    pub share_count: u32,
    /// Timestamp of the first share in this retarget window
    pub window_start: Option<Instant>,
    /// Timestamp of the last share
    pub last_share: Option<Instant>,
}

impl VardiffState {
    /// Create a new vardiff state with the given initial difficulty.
    pub fn new(initial_difficulty: f64) -> Self {
        Self {
            difficulty: initial_difficulty,
            share_count: 0,
            window_start: None,
            last_share: None,
        }
    }

    /// Record a share and optionally return a new difficulty if retarget is needed.
    /// Returns Some(new_difficulty) if difficulty should change, None otherwise.
    pub fn on_share(&mut self, config: &VardiffConfig) -> Option<f64> {
        let now = Instant::now();

        if self.window_start.is_none() {
            self.window_start = Some(now);
        }
        self.last_share = Some(now);
        self.share_count += 1;

        // Check if we've reached the retarget threshold
        if self.share_count < config.retarget_shares {
            return None;
        }

        // Calculate the time elapsed since the window started
        let elapsed = now
            .duration_since(self.window_start.unwrap())
            .as_secs_f64();

        // Avoid division by zero
        if elapsed < 0.001 {
            self.reset_window();
            return None;
        }

        // Calculate average time between shares
        let avg_time = elapsed / self.share_count as f64;

        // Calculate the adjustment ratio
        let ratio = avg_time / config.target_time;

        // Clamp the adjustment to 0.25x - 4x
        let clamped_ratio = ratio.max(0.25).min(4.0);

        // If the adjustment is within 0.85-1.15, skip (close enough)
        if clamped_ratio >= 0.85 && clamped_ratio <= 1.15 {
            self.reset_window();
            return None;
        }

        // Calculate new difficulty
        // If shares are coming too fast (avg_time < target), increase difficulty
        // If shares are coming too slow (avg_time > target), decrease difficulty
        // ratio > 1 means too slow => decrease diff. ratio < 1 means too fast => increase diff.
        // new_diff = old_diff / ratio (inversely proportional)
        let new_diff = self.difficulty / clamped_ratio;

        // Clamp to min/max
        let new_diff = new_diff.max(config.min_difficulty).min(config.max_difficulty);

        // Reset window
        self.reset_window();

        // Only return if actually different
        if (new_diff - self.difficulty).abs() / self.difficulty > 0.01 {
            self.difficulty = new_diff;
            Some(new_diff)
        } else {
            None
        }
    }

    /// Reset the retarget window.
    fn reset_window(&mut self) {
        self.share_count = 0;
        self.window_start = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_vardiff_no_retarget_before_threshold() {
        let config = VardiffConfig {
            retarget_shares: 12,
            target_time: 10.0,
            ..Default::default()
        };
        let mut state = VardiffState::new(1.0);

        // Submit fewer than retarget_shares
        for _ in 0..11 {
            assert!(state.on_share(&config).is_none());
        }
    }

    #[test]
    fn test_vardiff_state_resets() {
        let config = VardiffConfig {
            retarget_shares: 2,
            target_time: 10.0,
            ..Default::default()
        };
        let mut state = VardiffState::new(1.0);

        // First window
        state.on_share(&config);
        let _ = state.on_share(&config);
        // Window should have reset
        assert_eq!(state.share_count, 0);
    }
}
