use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FillCondition {
    #[default]
    Either,
    Fill,
    NoFill,
}

impl FillCondition {
    pub const ALL: [Self; 3] = [Self::Either, Self::Fill, Self::NoFill];
    pub fn label(self) -> &'static str {
        match self {
            Self::Either => "Any pass",
            Self::Fill => "Fill only",
            Self::NoFill => "Outside fills",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StepRule {
    pub chance: u8,
    pub repeats: u8,
    pub every: u8,
    pub fill: FillCondition,
}

impl Default for StepRule {
    fn default() -> Self {
        Self {
            chance: 100,
            repeats: 1,
            every: 1,
            fill: FillCondition::Either,
        }
    }
}

impl StepRule {
    pub fn validate(&mut self) {
        self.chance = self.chance.min(100);
        self.repeats = self.repeats.clamp(1, 4);
        self.every = self.every.clamp(1, 16);
    }

    pub fn hits(
        self,
        start: u64,
        next: u64,
        row: usize,
        cycle: u64,
        fill: bool,
    ) -> impl Iterator<Item = (u64, u64)> {
        let seed = (start ^ (row as u64).rotate_left(17) ^ cycle.rotate_left(31))
            .wrapping_add(0x9e37_79b9_7f4a_7c15);
        let seed = (seed ^ (seed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        let seed = (seed ^ (seed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        let roll = (seed ^ (seed >> 31)) % 100;
        let enabled = roll < u64::from(self.chance)
            && cycle.is_multiple_of(u64::from(self.every.max(1)))
            && match self.fill {
                FillCondition::Either => true,
                FillCondition::Fill => fill,
                FillCondition::NoFill => !fill,
            };
        let repeats = u64::from(self.repeats.clamp(1, 4));
        let span = next.saturating_sub(start).max(1);
        (0..if enabled { repeats } else { 0 }).map(move |index| {
            let offset = span * index / repeats;
            let length = if repeats == 1 {
                55
            } else {
                (span / repeats).saturating_sub(1).clamp(1, 55)
            };
            (start + offset, length)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fill_ratchets_use_the_swung_cell_and_skip_ineligible_passes() {
        let rule = StepRule {
            chance: 100,
            repeats: 3,
            every: 2,
            fill: FillCondition::Fill,
        };
        let hits = rule.hits(75, 120, 2, 2, true).collect::<Vec<_>>();
        assert_eq!(hits, [(75, 14), (90, 14), (105, 14)]);
        assert_eq!(rule.hits(75, 120, 2, 1, true).count(), 0);
        assert_eq!(rule.hits(75, 120, 2, 2, false).count(), 0);
        assert_eq!(
            StepRule { chance: 0, ..rule }
                .hits(75, 120, 2, 2, true)
                .count(),
            0
        );
    }
}
