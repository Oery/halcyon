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
        3.0 - 2.85 * self.skill()
    }

    /// Heading error accepted before committing to a jump, in degrees.
    pub fn aim_tolerance(self) -> f32 {
        6.0 - 4.5 * self.skill()
    }

    /// How precisely the run-up point is reached before a jump, in blocks.
    pub fn approach_tolerance(self) -> f64 {
        f64::from(0.12 - 0.08 * self.skill())
    }

    /// Probability that a jump press lands one tick late.
    pub fn timing_slip(self) -> f64 {
        f64::from(0.3 - 0.295 * self.skill())
    }

    /// Extra planning cost of an edge per unit of missing robustness, in ticks.
    pub fn risk_weight(self) -> f32 {
        15.0 + 90.0 * (1.0 - self.skill())
    }

    /// Ticks between seeing an opponent move and reacting to it.
    pub fn reaction_ticks(self) -> u32 {
        6 - (3.0 * self.skill()).round() as u32
    }

    /// Mean ticks between an opponent's move and the reaction to it; each
    /// decision draws the whole ticks around it.
    pub fn reaction(self) -> f32 {
        6.0 - 3.0 * self.skill()
    }

    /// Attack charge to wait for before the next swing, drawn per swing.
    pub fn click_threshold(self, rng: &mut Rng) -> f32 {
        let mean = 0.8 + 0.18 * self.skill();
        (mean + rng.normal() as f32 * (0.08 - 0.06 * self.skill())).clamp(0.5, 1.05)
    }

    /// Per-tick chance of clicking early, before the swing has recharged.
    pub fn spam_chance(self) -> f64 {
        f64::from(0.12 * (1.0 - self.skill()))
    }

    /// Chance of releasing forward after a sprinting hit, to sprint-hit again.
    pub fn wtap_chance(self) -> f64 {
        f64::from(0.15 + 0.75 * self.skill())
    }

    /// Chance of jumping in to land a critical hit.
    pub fn crit_jump_chance(self) -> f64 {
        f64::from(0.1 + 0.5 * self.skill())
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
