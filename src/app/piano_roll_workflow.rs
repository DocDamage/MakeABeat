use super::App;
use crate::{
    audio::PadMode,
    model::{ChopInput, PIANO_HIGH, PlayedChop, Quantize, STEPS, Studio, instrument::HitSound},
};
use std::collections::{BTreeSet, HashMap, HashSet};

pub(crate) type NoteKey = (u32, ChopInput);

pub(crate) fn note_key(note: &PlayedChop) -> NoteKey {
    (note.tick, note.input)
}

pub(crate) fn assigned(studio: &Studio, input: ChopInput) -> bool {
    match input {
        ChopInput::Pad(pad) => {
            let pad = usize::from(pad);
            pad < STEPS && studio.slice(pad, f64::INFINITY).is_some()
        }
        ChopInput::Key(note) => studio.key_range(note, f64::INFINITY).is_some(),
    }
}

pub(crate) enum Edit<'a> {
    Move {
        ticks: i64,
        rows: i32,
        lanes: &'a [ChopInput],
    },
    Resize {
        note: NoteKey,
        delta: i64,
    },
    Start {
        note: NoteKey,
        tick: u32,
    },
    Delete,
    Velocity(u8),
    Hold(u32),
    Sound {
        note: NoteKey,
        sound: HitSound,
    },
    Draw(PlayedChop),
    Paste(Vec<PlayedChop>),
    Quantize {
        grid: Quantize,
        strength: u8,
    },
    Humanize {
        ticks: u8,
        velocity: u8,
    },
}

#[derive(Default)]
pub(crate) struct Outcome {
    changed: bool,
    pub added: usize,
    pub skipped: usize,
    pub message: Option<&'static str>,
}

impl Outcome {
    fn rejected(message: &'static str) -> Self {
        Self {
            message: Some(message),
            ..Self::default()
        }
    }
}

impl App {
    pub(crate) fn edit_piano_roll(
        &mut self,
        edit: Edit<'_>,
        selection: &mut BTreeSet<NoteKey>,
        group: bool,
    ) -> Outcome {
        let outcome = apply(&mut self.project.studio, edit, selection);
        if outcome.changed {
            self.observe_edits(group || self.recording);
        }
        outcome
    }
}

pub(super) fn apply(
    studio: &mut Studio,
    edit: Edit<'_>,
    selection: &mut BTreeSet<NoteKey>,
) -> Outcome {
    let limit = studio.pattern_ticks();
    match edit {
        Edit::Move { ticks, rows, lanes } => move_notes(studio, selection, ticks, rows, lanes),
        Edit::Resize { note, delta } => transform(studio, selection, |mut event| {
            if note_key(&event) == note && event.mode == PadMode::Gate {
                let length = i64::from(event.length.unwrap_or(34)).saturating_add(delta);
                event.length = Some(length.clamp(1, i64::from(limit)) as u32);
            }
            Some(event)
        }),
        Edit::Start { note, tick } => transform(studio, selection, |mut event| {
            if note_key(&event) == note {
                event.tick = tick.min(limit - 1);
            }
            Some(event)
        }),
        Edit::Delete => transform(studio, selection, |_| None),
        Edit::Velocity(velocity) => transform(studio, selection, |mut event| {
            event.velocity = velocity.clamp(1, 127);
            Some(event)
        }),
        Edit::Hold(length) => transform(studio, selection, |mut event| {
            if event.mode == PadMode::Gate {
                event.length = Some(length.clamp(1, limit));
            }
            Some(event)
        }),
        Edit::Sound { note, sound } => {
            let changed = studio
                .roll_notes()
                .iter()
                .any(|event| note_key(event) == note && event.sound != sound);
            if changed {
                studio.set_roll_sound(note.0, note.1, sound);
            }
            Outcome {
                changed,
                ..Outcome::default()
            }
        }
        Edit::Draw(note) => {
            if !assigned(studio, note.input) || note.tick >= limit {
                return Outcome::rejected("Assign a chop to this lane before drawing hits.");
            }
            let mut outcome = insert(studio, selection, [note]);
            if outcome.skipped > 0 {
                outcome.message = Some("A hit already starts here on this lane.");
            }
            outcome
        }
        Edit::Paste(notes) => {
            if notes.iter().any(|note| note.tick >= limit) {
                return Outcome::rejected("Add more bars before pasting this selection.");
            }
            insert(studio, selection, notes)
        }
        Edit::Quantize { grid, strength } => quantize(studio, selection, grid, strength),
        Edit::Humanize { ticks, velocity } => humanize(studio, selection, ticks, velocity),
    }
}

fn transform(
    studio: &mut Studio,
    selection: &mut BTreeSet<NoteKey>,
    mut edit: impl FnMut(PlayedChop) -> Option<PlayedChop>,
) -> Outcome {
    if selection.is_empty() {
        return Outcome::default();
    }
    studio.edit_roll_notes(|notes| {
        let mut after = Vec::with_capacity(notes.len());
        let mut selected = BTreeSet::new();
        let mut keys = HashSet::with_capacity(notes.len());
        for note in notes.iter() {
            let is_selected = selection.contains(&note_key(note));
            let edited = if is_selected { edit(note.clone()) } else { Some(note.clone()) };
            if let Some(edited) = edited {
                let key = note_key(&edited);
                if !keys.insert(key) {
                    return Outcome::rejected(
                        "Two hits would share the same lane and start. Move one before applying the edit.",
                    );
                }
                if is_selected {
                    selected.insert(key);
                }
                after.push(edited);
            }
        }
        after.sort_unstable_by_key(note_key);
        let changed = *notes != after;
        if changed {
            *notes = after;
            *selection = selected;
        }
        Outcome { changed, ..Outcome::default() }
    })
}

fn insert(
    studio: &mut Studio,
    selection: &mut BTreeSet<NoteKey>,
    additions: impl IntoIterator<Item = PlayedChop>,
) -> Outcome {
    studio.edit_roll_notes(|notes| {
        let mut occupied: HashSet<_> = notes.iter().map(note_key).collect();
        let mut selected = BTreeSet::new();
        let mut skipped = 0;
        for note in additions {
            let key = note_key(&note);
            if occupied.insert(key) {
                notes.push(note);
                selected.insert(key);
            } else {
                skipped += 1;
            }
        }
        let added = selected.len();
        if added > 0 {
            notes.sort_unstable_by_key(note_key);
            *selection = selected;
        }
        Outcome {
            changed: added > 0,
            added,
            skipped,
            message: None,
        }
    })
}

fn move_notes(
    studio: &mut Studio,
    selection: &mut BTreeSet<NoteKey>,
    ticks: i64,
    rows: i32,
    lanes: &[ChopInput],
) -> Outcome {
    let limit = studio.pattern_ticks();
    let notes = studio.roll_notes();
    let mut selected = notes
        .iter()
        .filter(|note| note.tick < limit && selection.contains(&note_key(note)));
    let Some(first) = selected.next() else {
        return Outcome::default();
    };
    let last = selected.next_back().unwrap_or(first);
    let ticks = ticks.clamp(-i64::from(first.tick), i64::from(limit - 1 - last.tick));
    let destination = |input| {
        if rows == 0 {
            return Some(input);
        }
        let row = lanes.iter().position(|lane| *lane == input)?;
        lanes.get(row.checked_add_signed(rows as isize)?).copied()
    };
    if rows != 0
        && notes
            .iter()
            .filter(|note| note.tick < limit && selection.contains(&note_key(note)))
            .any(|note| {
                destination(note.input)
                    .is_none_or(|input| note.expression.is_none() && !assigned(studio, input))
            })
    {
        return Outcome::rejected("Move notes to lanes with assigned chops.");
    }
    transform(studio, selection, |mut note| {
        if note.tick < limit {
            note.tick = (i64::from(note.tick) + ticks) as u32;
            note.input = destination(note.input).unwrap_or(note.input);
        }
        Some(note)
    })
}

fn quantize(
    studio: &mut Studio,
    selection: &mut BTreeSet<NoteKey>,
    grid: Quantize,
    strength: u8,
) -> Outcome {
    let loop_ticks = u64::from(studio.pattern_ticks());
    let swing = studio.swing;
    let strength = f64::from(strength) / 100.0;
    let occupied: HashSet<_> = studio.roll_notes().iter().map(note_key).collect();
    let proposals: HashMap<_, _> = selection
        .iter()
        .filter_map(|&(start, input)| {
            let snapped = grid.snap(f64::from(start), swing) as f64;
            let tick = (f64::from(start) + (snapped - f64::from(start)) * strength).round() as u64;
            let tick = (tick % loop_ticks) as u32;
            (tick != start).then_some(((start, input), (tick, input)))
        })
        .collect();
    let mut count = HashMap::new();
    for &destination in proposals.values() {
        *count.entry(destination).or_insert(0) += 1;
    }
    let mut collision = false;
    let mut outcome = transform(studio, selection, |mut note| {
        if let Some(&target) = proposals.get(&note_key(&note)) {
            if occupied.contains(&target) || count[&target] > 1 {
                collision = true;
            } else {
                note.tick = target.0;
            }
        }
        Some(note)
    });
    if collision {
        outcome.message =
            Some("Hits that would land together on the same lane kept their original timing.");
    }
    outcome
}

fn humanize(
    studio: &mut Studio,
    selection: &mut BTreeSet<NoteKey>,
    ticks: u8,
    velocity: u8,
) -> Outcome {
    let mut occupied: HashSet<_> = studio.roll_notes().iter().map(note_key).collect();
    let limit = studio.pattern_ticks() - 1;
    transform(studio, selection, |mut note| {
        let lane = match note.input {
            ChopInput::Pad(pad) => u32::from(pad),
            ChopInput::Key(note) => u32::from(PIANO_HIGH.saturating_sub(note)),
        };
        let seed = note
            .tick
            .wrapping_mul(0x9e37_79b9)
            .wrapping_add(lane.wrapping_mul(0x85eb_ca6b));
        let jitter = |seed: u32, range: u8| {
            let range = i32::from(range);
            (seed % (2 * range + 1) as u32) as i32 - range
        };
        let tick = note
            .tick
            .saturating_add_signed(jitter(seed, ticks))
            .min(limit);
        if occupied.insert((tick, note.input)) {
            note.tick = tick;
        }
        note.velocity =
            (i32::from(note.velocity) + jitter(seed.rotate_left(13), velocity)).clamp(1, 127) as u8;
        Some(note)
    })
}
