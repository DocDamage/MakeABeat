use super::{
    STEPS, Sequence, drum_velocity, encode_drum_velocity,
    grid_edit::{Cell, GridRect, LANES},
};

#[derive(Clone, PartialEq, Eq)]
pub struct Settings {
    pub seed: u64,
    pub density: i32,
    pub amount: i32,
    pub fill: i32,
    pub lanes: [bool; LANES],
    pub region: Option<(usize, GridRect)>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            seed: 1,
            density: 0,
            amount: 45,
            fill: 25,
            lanes: [false; LANES],
            region: None,
        }
    }
}

impl Settings {
    fn locked(&self, bar: usize, lane: usize, step: usize) -> bool {
        self.lanes[lane]
            || self
                .region
                .is_some_and(|(index, region)| index == bar && region.contains(Cell { lane, step }))
    }
}

fn draw(seed: &mut u64) -> i32 {
    *seed = seed.wrapping_add(0x9e3779b97f4a7c15);
    let n = (*seed ^ (*seed >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    let n = (n ^ (n >> 27)).wrapping_mul(0x94d049bb133111eb);
    ((n ^ (n >> 31)) % 100) as i32
}

pub fn generate(base: &Sequence, settings: &Settings) -> [Sequence; 4] {
    std::array::from_fn(|candidate| {
        let mut seed = settings.seed.wrapping_add(candidate as u64);
        let mut next = base.clone();
        for (bar_index, bar) in next.bars.iter_mut().enumerate() {
            let fill = draw(&mut seed) < settings.fill;
            // Keep the kick and snare anchors. Vary hats, toms, rim and clap.
            for drum in 2..=8 {
                let lane = drum + 2;
                for step in 0..STEPS {
                    if settings.locked(bar_index, lane, step) {
                        continue;
                    }
                    let key = (lane * STEPS + step) as u8;
                    let hit = &mut bar.drums[drum][step];
                    let turn = draw(&mut seed);
                    if turn >= settings.amount {
                        continue;
                    }
                    if *hit != 0 {
                        if settings.density < 0 && draw(&mut seed) < -settings.density / 2 {
                            *hit = 0;
                            bar.rules.remove(&key);
                            bar.feel.remove(&key);
                            bar.sounds.remove(&key);
                            bar.ids.remove(&key);
                        } else {
                            let velocity =
                                i32::from(drum_velocity(*hit)) + draw(&mut seed) % 27 - 13;
                            *hit = encode_drum_velocity(velocity.clamp(1, 127) as u8);
                        }
                    } else {
                        let natural = match drum {
                            7 if step.is_multiple_of(2) => 28,
                            8 if step % 4 == 2 => 8,
                            5 | 6 if step % 4 == 3 => 8,
                            _ => 0,
                        };
                        let fill = if fill && step >= 12 && matches!(drum, 2..=5 | 7) {
                            32
                        } else {
                            0
                        };
                        let density = (natural + fill + settings.density / 3).clamp(0, 80);
                        if draw(&mut seed) < density {
                            let velocity = if step.is_multiple_of(4) { 85 } else { 64 };
                            *hit = encode_drum_velocity((velocity + draw(&mut seed) % 19) as u8);
                        }
                    }
                }
            }
            let movable: Vec<_> = (0..STEPS)
                .filter(|&step| bar.pads[step].is_some() && !settings.locked(bar_index, 0, step))
                .collect();
            for pair in movable.chunks_exact(2) {
                if draw(&mut seed) >= settings.amount / 2 {
                    continue;
                }
                let (a, b) = (pair[0], pair[1]);
                bar.pads.swap(a, b);
                let first_feel = bar.feel.remove(&(a as u8));
                let second_feel = bar.feel.remove(&(b as u8));
                bar.feel.extend(first_feel.map(|feel| (b as u8, feel)));
                bar.feel.extend(second_feel.map(|feel| (a as u8, feel)));
                let first = bar.rules.remove(&(a as u8));
                let second = bar.rules.remove(&(b as u8));
                bar.rules.extend(first.map(|rule| (b as u8, rule)));
                bar.rules.extend(second.map(|rule| (a as u8, rule)));
                let first = bar.sounds.remove(&(a as u8));
                let second = bar.sounds.remove(&(b as u8));
                bar.sounds.extend(first.map(|sound| (b as u8, sound)));
                bar.sounds.extend(second.map(|sound| (a as u8, sound)));
                let first = bar.ids.remove(&(a as u8));
                let second = bar.ids.remove(&(b as u8));
                bar.ids.extend(first.map(|id| (b as u8, id)));
                bar.ids.extend(second.map(|id| (a as u8, id)));
            }
        }
        next
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SequenceBar, StepRule};

    #[test]
    fn candidates_are_reproducible_and_keep_locked_hits_and_rules() {
        let mut bar = SequenceBar::default();
        bar.pads[0] = Some(3);
        bar.pads[4] = Some(7);
        bar.drums[7][4] = 2;
        bar.rules.insert(
            0,
            StepRule {
                repeats: 2,
                ..StepRule::default()
            },
        );
        bar.rules.insert(
            (9 * STEPS + 4) as u8,
            StepRule {
                chance: 25,
                ..StepRule::default()
            },
        );
        let base = Sequence {
            bars: vec![bar],
            ..Sequence::default()
        };
        let hit = Cell { lane: 9, step: 4 };
        let mut settings = Settings {
            density: 100,
            amount: 100,
            fill: 100,
            region: Some((0, GridRect::between(hit, hit))),
            ..Settings::default()
        };
        settings.lanes[0] = true;
        let candidates = generate(&base, &settings);
        assert_eq!(candidates, generate(&base, &settings));
        assert_ne!(candidates[0], candidates[1]);
        for candidate in candidates {
            assert_eq!(candidate.bars[0].pads, base.bars[0].pads);
            assert_eq!(candidate.bars[0].drums[7][4], 2);
            assert_eq!(candidate.bars[0].rules, base.bars[0].rules);
            assert_eq!(candidate.performance, base.performance);
        }
        settings.amount = 0;
        assert_eq!(
            generate(&base, &settings),
            std::array::from_fn(|_| base.clone())
        );
    }
}
