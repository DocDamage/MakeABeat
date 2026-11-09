use crate::model::{PIANO_HIGH, PIANO_LOW};
use serde::{Deserialize, Serialize};

pub const ROOTS: [&str; 12] = [
    "C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scale {
    #[default]
    Chromatic,
    Major,
    Minor,
    MajorPentatonic,
    MinorPentatonic,
    Dorian,
}

impl Scale {
    pub const ALL: [Self; 6] = [
        Self::Chromatic,
        Self::Major,
        Self::Minor,
        Self::MajorPentatonic,
        Self::MinorPentatonic,
        Self::Dorian,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Chromatic => "Chromatic",
            Self::Major => "Major",
            Self::Minor => "Natural minor",
            Self::MajorPentatonic => "Major pentatonic",
            Self::MinorPentatonic => "Minor pentatonic",
            Self::Dorian => "Dorian",
        }
    }

    fn intervals(self) -> &'static [u8] {
        match self {
            Self::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
            Self::Major => &[0, 2, 4, 5, 7, 9, 11],
            Self::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Self::MajorPentatonic => &[0, 2, 4, 7, 9],
            Self::MinorPentatonic => &[0, 3, 5, 7, 10],
            Self::Dorian => &[0, 2, 3, 5, 7, 9, 10],
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Chord {
    #[default]
    Single,
    Triad,
    Seventh,
}

impl Chord {
    pub const ALL: [Self; 3] = [Self::Single, Self::Triad, Self::Seventh];
    pub fn label(self) -> &'static str {
        match self {
            Self::Single => "Single note",
            Self::Triad => "Triad",
            Self::Seventh => "Seventh",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Harmony {
    pub scale: Scale,
    pub root: u8,
    pub constrain: bool,
    pub chord: Chord,
}

impl Harmony {
    pub fn contains(self, note: u8) -> bool {
        self.scale
            .intervals()
            .contains(&((note % 12 + 12 - self.root % 12) % 12))
    }

    pub fn notes(self, note: u8) -> impl Iterator<Item = u8> {
        let note = if self.constrain && !self.contains(note) {
            (PIANO_LOW..=PIANO_HIGH)
                .filter(|&key| self.contains(key))
                .min_by_key(|&key| key.abs_diff(note))
                .unwrap_or(note)
        } else {
            note
        };
        let count = match self.chord {
            Chord::Single => 1,
            Chord::Triad => 3,
            Chord::Seventh => 4,
        };
        let mut notes = [None; 4];
        notes[0] = Some(note);
        if self.scale == Scale::Chromatic || !self.contains(note) {
            for (slot, interval) in notes.iter_mut().take(count).skip(1).zip([4, 7, 10]) {
                *slot = (note + interval <= PIANO_HIGH).then_some(note + interval);
            }
        } else {
            let mut in_scale = (note + 1..=PIANO_HIGH).filter(|&key| self.contains(key));
            for slot in notes.iter_mut().take(count).skip(1) {
                *slot = in_scale.nth(1);
            }
        }
        notes.into_iter().flatten()
    }
}
