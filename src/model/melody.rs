use serde::{Deserialize, Serialize};

pub const BEND_RANGE: i16 = 1_200;
pub const MAX_POINTS: usize = 16_384;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Instrument {
    #[default]
    Sine,
    Sample {
        source: usize,
        start: u32,
        end: u32,
        root: u8,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bend {
    pub tick: u32,
    pub cents: i16,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Expression {
    pub instrument: Instrument,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bends: Vec<Bend>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phrase: Option<u64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub continuation: bool,
}

impl Expression {
    pub fn valid(&self) -> bool {
        self.bends.len() <= MAX_POINTS
            && self
                .bends
                .windows(2)
                .all(|pair| pair[0].tick < pair[1].tick)
            && self
                .bends
                .iter()
                .all(|point| (-BEND_RANGE..=BEND_RANGE).contains(&point.cents))
            && match self.instrument {
                Instrument::Sine => true,
                Instrument::Sample {
                    start, end, root, ..
                } => start < end && (super::PIANO_LOW..=super::PIANO_HIGH).contains(&root),
            }
    }

    pub fn has_bends(&self, length: u32) -> bool {
        length > 0
            && (self
                .bends
                .iter()
                .take_while(|point| point.tick < length)
                .any(|point| point.cents != 0)
                || cents_at(&self.bends, f64::from(length - 1)) != 0.0
                || cents_at(&self.bends, 0.0) != 0.0)
    }
}

pub fn cents_at(points: &[Bend], tick: f64) -> f64 {
    let i = points.partition_point(|point| f64::from(point.tick) <= tick);
    match (i.checked_sub(1).and_then(|i| points.get(i)), points.get(i)) {
        (Some(a), Some(b)) => {
            let mix = (tick - f64::from(a.tick)) / f64::from(b.tick - a.tick);
            f64::from(a.cents) + f64::from(b.cents - a.cents) * mix
        }
        (Some(point), None) | (None, Some(point)) => f64::from(point.cents),
        (None, None) => 0.0,
    }
}

pub fn trim(points: &[Bend], start: u32, end: u32) -> Vec<Bend> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut result = vec![Bend {
        tick: 0,
        cents: cents_at(points, f64::from(start)).round() as i16,
    }];
    result.extend(
        points
            .iter()
            .filter(|point| start < point.tick && point.tick < end)
            .map(|point| Bend {
                tick: point.tick - start,
                cents: point.cents,
            }),
    );
    result.push(Bend {
        tick: end - start,
        cents: cents_at(points, f64::from(end)).round() as i16,
    });
    if result.iter().all(|point| point.cents == 0) {
        result.clear();
    }
    result
}

pub fn join_continuations(notes: &mut Vec<super::PlayedChop>) {
    if !notes.iter().any(|note| {
        note.expression
            .as_ref()
            .is_some_and(|expression| expression.continuation)
    }) {
        return;
    }
    use std::{collections::HashMap, sync::Arc};
    let mut output: Vec<super::PlayedChop> = Vec::with_capacity(notes.len());
    let mut active: HashMap<u64, usize> = HashMap::new();
    for note in notes.drain(..) {
        if let Some(expression) = &note.expression
            && let Some(chain) = expression.chain
        {
            if expression.continuation
                && let Some(previous) = active.get(&chain).and_then(|&i| output.get_mut(i))
                && let Some(length) = previous.length
                && previous.tick + length == note.tick
                && previous.input == note.input
                && previous.velocity == note.velocity
                && previous.sound == note.sound
                && let Some(head) = &mut previous.expression
                && head.instrument == expression.instrument
            {
                let tail = note.length.unwrap_or(1);
                let head = Arc::make_mut(head);
                if !head.bends.is_empty() || !expression.bends.is_empty() {
                    let mut joined = trim(&head.bends, 0, length);
                    if joined.is_empty() {
                        joined.push(Bend { tick: 0, cents: 0 });
                    }
                    joined.retain(|point| point.tick < length);
                    let bends = trim(&expression.bends, 0, tail);
                    if bends.is_empty() {
                        joined.push(Bend {
                            tick: length,
                            cents: 0,
                        });
                    }
                    joined.extend(bends.into_iter().map(|point| Bend {
                        tick: point.tick + length,
                        ..point
                    }));
                    head.bends = joined;
                }
                previous.length = Some(length + tail);
                continue;
            }
            active.insert(chain, output.len());
        }
        output.push(note);
    }
    *notes = output;
}
