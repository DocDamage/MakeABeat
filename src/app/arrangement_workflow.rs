use super::App;
use crate::model::{MAX_SECTIONS, MAX_SONG_BARS, Section, SectionKind, SectionSpan, Studio};

#[derive(Clone, Copy)]
pub(crate) enum Edge {
    Start,
    End,
}

#[derive(Clone, Copy)]
pub(crate) enum Edit {
    Add(usize),
    Duplicate(usize),
    Move {
        from: usize,
        to: usize,
    },
    Trim {
        index: usize,
        edge: Edge,
        delta: i32,
    },
}

// The preview and the edit share this calculation. Positive deltas move the
// handle to the right.
pub(crate) fn trimmed_span(
    studio: &Studio,
    index: usize,
    edge: Edge,
    delta: i32,
) -> Option<SectionSpan> {
    let section = studio.sections.get(index)?;
    let length = studio.patterns.get(section.pattern)?.length;
    if length == 0 {
        return None;
    }
    let previous = section.bars(length);
    let available = MAX_SONG_BARS
        .saturating_sub(studio.song_bars() - previous)
        .max(previous);
    let change = match edge {
        Edge::Start => -i64::from(delta),
        Edge::End => i64::from(delta),
    };
    let bars = (previous as i64 + change).clamp(1, available as i64) as usize;
    if bars == previous {
        return None;
    }
    let mut offset = section.offset(length) as i64;
    if matches!(edge, Edge::Start) {
        offset += previous as i64 - bars as i64;
    }
    Some(SectionSpan {
        bars,
        offset: offset.rem_euclid(length as i64) as usize,
    })
}

impl App {
    fn can_insert_arrangement_bars(&self, bars: usize) -> bool {
        !self.recording
            && self.song_playing.is_none()
            && self.project.studio.sections.len() < MAX_SECTIONS
            && self.project.studio.song_bars() + bars <= MAX_SONG_BARS
    }

    pub(crate) fn can_add_section(&self, pattern: usize) -> bool {
        self.project.studio.patterns.get(pattern).is_some_and(|pattern| {
            pattern.length > 0 && self.can_insert_arrangement_bars(pattern.length)
        })
    }

    pub(crate) fn can_duplicate_section(&self, index: usize) -> bool {
        self.project.studio.sections.get(index).is_some_and(|section| {
            self.project.studio
                .patterns
                .get(section.pattern)
                .is_some_and(|pattern| {
                    pattern.length > 0
                        && self.can_insert_arrangement_bars(section.bars(pattern.length))
                })
        })
    }

    pub(crate) fn edit_arrangement(&mut self, edit: Edit) {
        if self.song_playing.is_some() || self.recording {
            return;
        }
        let count = self.project.studio.sections.len();
        let selected = match edit {
            Edit::Add(pattern) if self.can_add_section(pattern) => {
                let kind = match count {
                    0 => SectionKind::Intro,
                    2 => SectionKind::Hook,
                    _ => SectionKind::Verse,
                };
                self.project.studio.sections.push(Section::new(kind, pattern, 1));
                count
            }
            Edit::Duplicate(index) if self.can_duplicate_section(index) => {
                let section = self.project.studio.sections[index].clone();
                self.project.studio.sections.insert(index + 1, section);
                index + 1
            }
            Edit::Move { from, to } if from < count && to < count && from != to => {
                let section = self.project.studio.sections.remove(from);
                self.project.studio.sections.insert(to, section);
                to
            }
            Edit::Trim { index, edge, delta } => {
                let Some(span) = trimmed_span(&self.project.studio, index, edge, delta) else {
                    return;
                };
                self.project.studio.sections[index].span = Some(span);
                index
            }
            _ => return,
        };
        self.view.arrangement_timeline.sections_changed();
        self.view.arrangement_timeline.select(selected);
        self.session.end_group();
        self.observe_edits(false);
        self.ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TICKS_PER_BAR;

    #[test]
    fn adding_a_section_updates_playback_and_respects_edit_limits() {
        let mut app = App::new(eframe::egui::Context::default());
        app.audio.remove_output();
        app.project.studio.save_pattern("Beat", false);
        app.session.reset(app.project.clone(), None, None);

        app.song_playing = Some(1);
        app.edit_arrangement(Edit::Add(0));
        app.song_playing = None;
        app.edit_arrangement(Edit::Add(1));
        assert!(app.project.studio.sections.is_empty());
        assert!(!app.can_undo_project(false));

        app.edit_arrangement(Edit::Add(0));
        assert_eq!(app.project.studio.sections[0].kind, SectionKind::Intro);
        assert_eq!(app.view.arrangement_timeline.selected, Some(0));
        assert_eq!(app.project.studio.section_range(0), Some(0..TICKS_PER_BAR));
        assert_eq!(app.project.studio.song_sequence().unwrap().ticks(), TICKS_PER_BAR);
        app.undo_project(false);
        assert!(app.project.studio.song_sequence().is_none());
        assert_eq!(app.view.arrangement_timeline.selected, None);
        app.undo_project(true);
        assert_eq!(app.view.arrangement_timeline.selected, Some(0));

        app.edit_arrangement(Edit::Duplicate(0));
        app.edit_arrangement(Edit::Trim {
            index: 1,
            edge: Edge::End,
            delta: 1,
        });
        app.undo_project(false);
        assert_eq!(app.project.studio.sections[0], app.project.studio.sections[1]);
        app.undo_project(true);
        assert_eq!(app.view.arrangement_timeline.selected, Some(1));
        assert_eq!(
            app.project.studio.sections[1].span,
            Some(SectionSpan { offset: 0, bars: 2 })
        );
        app.undo_project(false);
        app.undo_project(false);

        app.edit_arrangement(Edit::Trim {
            index: 0,
            edge: Edge::End,
            delta: i32::MAX,
        });
        assert_eq!(app.project.studio.song_bars(), MAX_SONG_BARS);
        app.edit_arrangement(Edit::Duplicate(0));
        app.edit_arrangement(Edit::Add(0));
        assert_eq!(app.project.studio.sections.len(), 1);
        app.undo_project(false);
        assert_eq!(app.project.studio.song_bars(), 1);
    }
}
