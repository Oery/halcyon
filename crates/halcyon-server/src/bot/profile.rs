//! Fixed-at-creation skill parameters. Every knob is a pure function of the
//! level so behavior is reproducible from `(level, seed)`.

use super::rng::Rng;

#[derive(Clone, Copy, Debug)]
pub struct BotProfile {
    /// 0 is a beginner, 100 a strong player.
    pub level: u8,
}

impl BotProfile {
    pub fn new(level: u8) -> Self {
        Self { level: level.min(100) }
    }

    fn skill(self) -> f32 {
        f32::from(self.level) / 100.0
    }

    /// Maximum camera turn per tick, in degrees.
    pub fn turn_rate(self) -> f32 {
        12.0 + 24.0 * self.skill()
    }

    /// Fraction of the remaining turn covered per tick.
    pub fn turn_easing(self) -> f32 {
        0.25 + 0.2 * self.skill()
    }

    /// Camera tremor amplitude, in degrees.
    pub fn tremor(self) -> f32 {
        0.35 - 0.25 * self.skill()
    }

    /// Standard deviation of the heading error committed to a jump, in degrees.
    pub fn aim_sigma(self) -> f32 {
        3.0 - 2.6 * self.skill()
    }

    /// Heading error accepted before committing to a jump, in degrees.
    pub fn aim_tolerance(self) -> f32 {
        6.0 - 4.5 * self.skill()
    }

    /// Probability that a jump press lands one tick late.
    pub fn timing_slip(self) -> f64 {
        f64::from(0.3 - 0.27 * self.skill())
    }

    /// Extra planning cost of an edge per unit of missing robustness, in ticks.
    pub fn risk_weight(self) -> f32 {
        15.0 + 90.0 * (1.0 - self.skill())
    }

    /// Pause before a jump: longer for hard jumps and weaker players.
    pub fn hesitation(self, robustness: f32, rng: &mut Rng) -> u16 {
        let difficulty = (1.0 - robustness.clamp(0.0, 1.0)) * 25.0 * (1.0 - 0.6 * self.skill());
        (rng.ticks(1, 6) as f32 + difficulty * rng.range(0.5, 1.5) as f32) as u16
    }
}

/// `HALCYON_BOT_LEVEL` is either `n` or `min-max`; levels are drawn once per bot.
pub fn parse_levels(value: Option<&str>) -> (u8, u8) {
    let parse = |s: &str| s.trim().parse::<u8>().ok().map(|n| n.min(100));
    match value.map(|v| v.split_once('-').map_or((v, v), |(a, b)| (a, b))) {
        Some((a, b)) => match (parse(a), parse(b)) {
            (Some(a), Some(b)) => (a.min(b), a.max(b)),
            _ => (20, 90),
        },
        None => (20, 90),
    }
}
