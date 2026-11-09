use super::{MAX_BARS, STEPS, SequenceBar, StepRule, encode_drum_velocity};

pub const FAMILIES: [&str; 4] = [
    "Hip-hop & R&B",
    "Dance & electronic",
    "Rock, pop, funk & jazz",
    "Caribbean, Latin & African",
];

#[derive(Clone, Copy)]
#[repr(usize)]
enum Lane {
    Kick = 0,
    Snare = 1,
    LowTom = 2,
    Rim = 5,
    Clap = 6,
    Hat = 7,
    OpenHat = 8,
    Crash = 9,
    Ride = 10,
}

pub struct Template {
    pub name: &'static str,
    pub genre: &'static str,
    pub family: usize,
    pub bpm: u16,
    pub swing: f32,
    pub description: &'static str,
    pub bars: &'static [DrumBar],
}

impl Template {
    pub fn end_bar(&self, start: usize) -> Option<usize> {
        start
            .checked_add(self.bars.len())
            .filter(|&end| end <= MAX_BARS)
    }
}

pub struct DrumBar {
    pub label: &'static str,
    lanes: &'static [(Lane, &'static [u8; 19])],
}

impl DrumBar {
    pub fn steps(&self) -> SequenceBar {
        let mut bar = SequenceBar::default();
        for &(lane, pattern) in self.lanes {
            let lane = lane as usize;
            for step in 0..STEPS {
                let (velocity, repeats) = match pattern[step + step / 4] {
                    b'.' => continue,
                    b'g' => (42, 1),
                    b'x' => (89, 1),
                    b'X' => (120, 1),
                    roll @ b'2'..=b'4' => (76, roll - b'0'),
                    _ => unreachable!("invalid drum template step"),
                };
                bar.drums[lane][step] = encode_drum_velocity(velocity);
                if repeats > 1 {
                    bar.rules.insert(
                        ((lane + 2) * STEPS + step) as u8,
                        StepRule {
                            repeats,
                            ..StepRule::default()
                        },
                    );
                }
            }
        }
        bar
    }
}

macro_rules! bar {
    ($label:literal; $($lane:ident $pattern:literal),+ $(,)?) => {
        DrumBar { label: $label, lanes: &[$((Lane::$lane, $pattern)),+] }
    };
}

macro_rules! phrase {
    ($family:literal, $genre:literal, $name:literal, $bpm:literal, $swing:literal, $description:literal;
        $($bar:expr),+ $(,)?) => {
        Template {
            name: $name,
            genre: $genre,
            family: $family,
            bpm: $bpm,
            swing: $swing,
            description: $description,
            bars: &[$($bar),+],
        }
    };
}

macro_rules! beat {
    ($family:literal, $genre:literal, $name:literal, $bpm:literal, $swing:literal, $description:literal;
        $($lane:ident $pattern:literal),+ $(,)?) => {
        phrase!($family, $genre, $name, $bpm, $swing, $description;
            bar!("Groove"; $($lane $pattern),+))
    };
}

// Each group is one beat. Uppercase hits are accented, g marks ghost notes,
// and 2 through 4 repeat a hit within its step.
pub const TEMPLATES: &[Template] = &[
    beat!(0, "Boom bap", "Boom bap", 92, 0.22, "A heavy backbeat with offbeat kicks and even hats.";
        Kick b"X... ..x. ..x. ....", Snare b".... X... .... X...", Hat b"X.x. x.x. X.x. x.x."),
    beat!(0, "Boom bap", "Bare backbeat", 88, 0.0, "Just the kick, snare and quiet quarter-note hats.";
        Kick b"X... .... X... ....", Snare b".... X... .... X...", Hat b"x... g... x... g..."),
    beat!(0, "Boom bap", "Double-kick pickup", 94, 0.18, "Two short kicks lean into the second snare.";
        Kick b"X... ..x. ..xx ....", Snare b".... X... .... X...", Hat b"X.x. ..x. X.x. x.x."),
    beat!(0, "Boom bap", "Behind the pocket", 87, 0.38, "Soft sixteenths and an offbeat kick make a loose pocket.";
        Kick b"X... .x.. ...x ....", Snare b".... X... ..g. X...", Hat b"X..g .x.g X..g .x.."),
    beat!(0, "Boom bap", "Stutter-step kick", 96, 0.16, "A tight pair of opening kicks leaves the rest of the bar open.";
        Kick b"Xx.. .... ..x. ....", Snare b".... X... .... X..g", Hat b"X.x. x... ..x. X.x."),
    beat!(0, "Boom bap", "Ghost notes", 91, 0.25, "Quiet snare taps weave around a strong two and four.";
        Kick b"X... ..x. x..x ....", Snare b"...g X..g .g.. X.g.", Hat b"X.x. x.x. x.x. x.x."),
    beat!(0, "Boom bap", "Open-hat lift", 98, 0.12, "An open hat lifts the backbeat and carries into the next bar.";
        Kick b"X... ...x X... .x..", Snare b".... X... .... X...", Hat b"x.x. x... x.x. x...", OpenHat b".... ..x. .... ..X."),
    beat!(0, "Boom bap", "Rim pocket", 84, 0.32, "A rim backbeat leaves space for a dry, sparse sample flip.";
        Kick b"X... ..x. ...x ....", Rim b".... X... ..g. X...", Hat b"x..g ..x. x... ..g."),
    beat!(0, "Boom bap", "Kick on the and", 93, 0.20, "Offbeat kicks answer the downbeat without crowding the snare.";
        Kick b"X.x. ..x. ..X. ....", Snare b".... X... .... X...", Hat b"X..g x.x. X... x.x."),
    beat!(0, "Boom bap", "Pushed backbeat", 100, 0.05, "The second snare arrives a sixteenth ahead of beat four.";
        Kick b"X... ..x. X... ..x.", Snare b".... X... ...X ....", Hat b"X.x. x.x. X.x. x.x."),
    beat!(0, "Boom bap", "Late backbeat", 89, 0.15, "A delayed second snare turns over the bar.";
        Kick b"X... ..x. X... ...x", Snare b".... X... .... .X..", Hat b"X.x. ..x. X.x. ..x."),
    beat!(0, "Boom bap", "Drum break pocket", 102, 0.12, "A busy kick and a short open hat suggest a chopped drum break.";
        Kick b"X..x ..x. X..x ....", Snare b"...g X... .g.. X..g", Hat b"X.x. x.x. X.x. x...", OpenHat b".... .... .... ..x."),
    beat!(0, "Boom bap", "Straight cypher", 104, 0.0, "Even hats and square snares frame a clipped kick rhythm.";
        Kick b"X.x. ...x X... ..x.", Snare b".... X... .... X...", Hat b"X.x. x.x. X.x. x.x."),
    beat!(0, "Boom bap", "Ride pocket", 92, 0.20, "A light ride takes the lead above sparse closed hats.";
        Kick b"X... .x.. ..x. ....", Snare b".... X..g .... X...", Ride b"X.x. ..x. X.x. ..x.", Hat b".... x... .... x..."),
    beat!(0, "Boom bap", "Stop before four", 86, 0.27, "The hats drop out before a single snare closes the bar.";
        Kick b"X... ...x .x.. ....", Snare b".... X... .... X...", Hat b"X.g. x..g X.g. ...."),
    beat!(0, "Boom bap", "Tom turnaround", 95, 0.18, "Low toms answer the final snare, ready for a bar change.";
        Kick b"X... ..x. X... ....", Snare b".... X... ..g. X...", Hat b"X.x. x.x. X.x. x...", LowTom b".... .... .... .x.X"),
    beat!(0, "Boom bap", "Fast break flip", 108, 0.08, "Quick ghost snares and alternating hat accents cut through a fast loop.";
        Kick b"X..x .... X.xx ....", Snare b"...g X.g. ..g. X...", Hat b"Xgxg x.x. Xgxg x.x."),
    phrase!(0, "Boom bap", "Crate-digger turnaround", 92, 0.20, "A steady backbeat answers itself with a double kick and a short snare fill.";
        bar!("Pocket"; Kick b"X... ..x. X... ....", Snare b".... X... .... X...", Hat b"X.x. x.x. X.x. x.x."),
        bar!("Snare fill"; Kick b"X... ...x ..xx ....", Snare b".... X... ..g. X.g2", Hat b"X.x. x... X.x. x...")),
    phrase!(0, "Boom bap", "Rim and tom answer", 85, 0.30, "A sparse rim groove opens into a low-tom answer.";
        bar!("Rim groove"; Kick b"X... .x.. ..x. ....", Rim b".... X... .... X...", Hat b"x..g ..x. x..g ..x."),
        bar!("Tom fill"; Kick b"X... ..x. ...x ....", Rim b".... X... ..g. X...", Hat b"x... ..x. x..g ....", LowTom b".... .... .... .gxX")),
    phrase!(0, "Boom bap", "Chopped break answer", 104, 0.10, "An open-hat pickup leads to a busier broken kick and snare turnaround.";
        bar!("Break groove"; Kick b"X..x ..x. X... ....", Snare b"...g X... .... X...", Hat b"X.x. x.x. X.x. x...", OpenHat b".... .... .... ..x."),
        bar!("Snare turnaround"; Kick b"X... ...x X.x. ..x.", Snare b"..g. X..g ...g X.gx", Hat b"X.x. x.x. X... x...")),
    phrase!(0, "Boom bap", "Four-bar cipher", 94, 0.18, "The kick shifts through three bars before a snare and tom turnaround.";
        bar!("Main pocket"; Kick b"X... ..x. X.x. ....", Snare b".... X... .... X...", Hat b"X.x. x.x. X.x. x.x."),
        bar!("Kick pickup"; Kick b"X... ...x X... ..x.", Snare b".... X... ..g. X...", Hat b"X.x. ..x. X.x. x.x."),
        bar!("Open-hat lift"; Kick b"X... ..x. X..x ....", Snare b".... X... .... X...", Hat b"X.x. x... X.x. x...", OpenHat b".... ..x. .... ..x."),
        bar!("Snare and tom fill"; Kick b"X... .x.. ..x. ....", Snare b".... X... ...g Xg..", Hat b"X.x. x.x. X.x. x...", LowTom b".... .... .... ..xX")),
    phrase!(0, "Boom bap", "Loose pocket suite", 88, 0.32, "Ghost notes and rim accents build to a late snare fill with a ride on top.";
        bar!("Loose groove"; Kick b"X... .x.. ...x ....", Snare b".... X... .... X...", Hat b"X..g .x.g X..g .x.."),
        bar!("Ghost snare"; Kick b"X... ..x. ..x. ....", Snare b"...g X..g .g.. X...", Hat b"x..g .x.. X..g .x.."),
        bar!("Rim and ride"; Kick b"X... ...x x... ..x.", Rim b".... X... .... X...", Hat b"x... .... x... ....", Ride b"X..g ..x. X... ..x."),
        bar!("Late snare fill"; Kick b"X... ..x. .x.. ....", Snare b".... X... ..g. .Xg2", Ride b"X..g ..x. X..g ....")),
    phrase!(0, "Boom bap", "Break flip sequence", 106, 0.08, "Alternating chopped kicks, a third-bar break and a kick-snare fill.";
        bar!("Chopped break"; Kick b"X..x ..x. X... ..x.", Snare b"...g X... .... X...", Hat b"Xgxg x.x. Xgxg x.x."),
        bar!("Open hats"; Kick b"X... .x.. X.x. ....", Snare b".... X..g ..g. X...", Hat b"X.x. x... X.x. x...", OpenHat b".... ..x. .... ..x."),
        bar!("Breakdown"; Kick b"X... .... ..x. ....", Snare b".... X... .... X...", Hat b"x... .... x... ...."),
        bar!("Kick-snare fill"; Kick b"X..x ..x. X... ..xx", Snare b".... X... .g.. Xxg.", Hat b"X.x. x.x. X.x. x...")),
    beat!(0, "Lo-fi hip-hop", "Laid back", 78, 0.30, "Soft hats and a late-feeling kick pattern leave room for a sample.";
        Kick b"X... ...x ..x. ....", Snare b".... X... ...g X...", Hat b"..x. .g.. x..g ..x."),
    beat!(0, "Trap", "Half-time trap", 145, 0.0, "Half-time clap, syncopated kick and tight hat rolls.";
        Kick b"X... ...x .... ..x.", Clap b".... .... X... ....", Hat b"X.x. x.x2 X.x. x.3.", OpenHat b".... ..x. .... ...."),
    beat!(0, "Trap", "Sparse 808 pocket", 138, 0.0, "Two spaced kicks and a single clap leave a wide half-time pocket.";
        Kick b"X... .... .... .x..", Clap b".... .... X... ....", Hat b"x... ..g. x... ..g."),
    beat!(0, "Trap", "Straight eighth hats", 146, 0.0, "A steady eighth-note hat over a short kick phrase.";
        Kick b"X... .x.. .... ..x.", Clap b".... .... X... ....", Hat b"X.x. x.x. X.x. x.x."),
    beat!(0, "Trap", "Sixteenth hats", 150, 0.0, "Alternating hat accents make the sixteenths cut through.";
        Kick b"X... ..x. .x.. ...x", Snare b".... .... X... ....", Hat b"Xgxg xgxg Xgxg xgxg"),
    beat!(0, "Trap", "Two-hit hat bursts", 144, 0.0, "Paired hat hits answer each other on beats two and four.";
        Kick b"X... ...x .... .x..", Clap b".... .... X... ....", Hat b"X.x. x.2. X.x. x.2."),
    beat!(0, "Trap", "Three-hit hat bursts", 142, 0.0, "Short three-hit bursts punctuate an open hat pattern.";
        Kick b"X... ..x. .... ...x", Clap b".... .... X... ....", Hat b"X.x3 .... X.x. ..3.", OpenHat b".... x... .... ...."),
    beat!(0, "Trap", "Four-hit hat bursts", 152, 0.0, "Fast hat repeats build into the clap and return at the end.";
        Kick b"X..x .... .x.. ..x.", Clap b".... .... X... ....", Hat b"X.x. x.x4 X.x. x.x4"),
    beat!(0, "Trap", "Hats with gaps", 134, 0.06, "Short hat phrases alternate with deliberate gaps.";
        Kick b"X... .x.. .... ..xx", Snare b".... .... X... ....", Hat b"Xxg. .... X.gx ...2"),
    beat!(0, "Trap", "Layered clap", 140, 0.0, "A snare reinforces the clap on three with a rim pickup.";
        Kick b"X... ..x. ..x. ....", Clap b".... .... X... ....", Snare b".... .... x... ....", Rim b".... ...g .... ....", Hat b"x.x. ..x. x.x. ..2."),
    beat!(0, "Trap", "Open-hat answer", 148, 0.0, "An open hat answers each kick phrase with a sharp release.";
        Kick b"X..x .... ..x. ...x", Clap b".... .... X... ....", Hat b"x... x.x. x... ..x.", OpenHat b"..X. .... .... X..."),
    beat!(0, "Trap", "Offbeat kick pocket", 136, 0.0, "The first kick lands after beat one while the clap holds the center.";
        Kick b"..X. ...x ..x. .x..", Clap b".... .... X... ....", Hat b"X.x. ..x. X.x. x..2"),
    beat!(0, "Trap", "Kick trip-up", 156, 0.0, "A cluster of kicks on beat four sends the pattern back to one.";
        Kick b"X... ..x. .... .xxx", Clap b".... .... X... ....", Hat b"X.x. x..g X.x. x.2."),
    beat!(0, "Trap", "Snare roll pickup", 154, 0.0, "A short snare roll leads into the half-time backbeat.";
        Kick b"X... .x.. ..x. ...x", Snare b".... ...3 X... ....", Hat b"X.x. x.x. ..x. x.x."),
    beat!(0, "Trap", "Beat-four rim", 141, 0.0, "A rim answer on beat four opens space around the clap.";
        Kick b"X... ...x ..x. ....", Clap b".... .... X... ....", Rim b".... .... .... x..g", Hat b"X..g .x.. X..g .x.2"),
    beat!(0, "Trap", "Low-tom accents", 147, 0.0, "Low toms punctuate the empty spaces between the kicks.";
        Kick b"X... ...x .... ..x.", Clap b".... .... X... ....", LowTom b".... x... ..x. ....", Hat b"x.x. x... x.x. ..3."),
    beat!(0, "Trap", "Last-beat roll", 160, 0.0, "An understated hat pulse gives way to a busy final beat.";
        Kick b"X... ..x. .x.. ...x", Snare b".... .... X... ....", Hat b"x... x... x... x234"),
    beat!(0, "Trap", "Double-time clap", 132, 0.0, "Two claps give this 808-style groove a more forward backbeat.";
        Kick b"X... ...x X..x ....", Clap b".... X... .... X...", Hat b"X.x. ..2. X.x. ..3.", OpenHat b".... .... ..x. ...."),
    phrase!(0, "Trap", "Hat roll call and response", 145, 0.0, "Two kick phrases trade short hat bursts, ending with a fast roll.";
        bar!("Call"; Kick b"X... ...x ..x. ....", Clap b".... .... X... ....", Hat b"X.x. x.x2 X.x. x.x."),
        bar!("Roll answer"; Kick b"X..x .... .x.. ...x", Clap b".... .... X... ....", Hat b"X.x. x... X.x. x.34", OpenHat b".... ..x. .... ....")),
    phrase!(0, "Trap", "808 pocket and pickup", 138, 0.0, "A sparse half-time pocket makes room for a snare pickup and a closing kick double.";
        bar!("Sparse pocket"; Kick b"X... .... .... ..x.", Clap b".... .... X... ....", Hat b"x... ..x. x... ..x."),
        bar!("Snare pickup"; Kick b"X... ..x. .... ..xx", Clap b".... .... X... ....", Snare b".... ...2 .... ...g", Hat b"x.x. ..x. x.x. ..3.")),
    phrase!(0, "Trap", "Clap switch", 152, 0.0, "A clap leads the first bar and a snare takes over for a final roll.";
        bar!("Clap groove"; Kick b"X... .x.. ..x. ....", Clap b".... .... X... ....", Hat b"X.x. x.x. X.x. x.x."),
        bar!("Snare fill"; Kick b"X... ...x .... .xx.", Snare b".... .... X... ..24", Hat b"X..g x.x. X..g x...", OpenHat b".... .... ..x. ....")),
    phrase!(0, "Trap", "808 runway", 142, 0.0, "The hats open up, drop away, then return in a four-hit roll.";
        bar!("Half-time groove"; Kick b"X... ...x .... ..x.", Clap b".... .... X... ....", Hat b"X.x. x.x. X.x. x.x."),
        bar!("Hat lift"; Kick b"X..x .... ..x. ....", Clap b".... .... X... ....", Hat b"X.x. x.2. X.x. x...", OpenHat b".... .... .... ..x."),
        bar!("Sparse break"; Kick b"X... .... .x.. ....", Clap b".... .... X... ....", Hat b"x... .... x... ..g."),
        bar!("Four-hit roll"; Kick b"X... ..x. .... .x.x", Clap b".... .... X... ....", Hat b"X.x. x.2. X.x. .x34")),
    phrase!(0, "Trap", "Snare roll arc", 156, 0.0, "Different kick and hat phrases lead to layered snare rolls in the fourth bar.";
        bar!("Straight hats"; Kick b"X... ..x. .x.. ....", Snare b".... .... X... ....", Hat b"X.x. x.x. X.x. x.x."),
        bar!("Kick variation"; Kick b"..X. ...x X... ..x.", Snare b".... .... X... ....", Hat b"X.x. ..2. X.x. ...x"),
        bar!("Rim answer"; Kick b"X... .... ..x. ...x", Snare b".... .... X... ....", Rim b".... .x.. .... x...", Hat b"X..g x... X..g x..."),
        bar!("Snare roll fill"; Kick b"X... .x.. .... ..xx", Snare b".... ...2 X... .x34", Clap b".... .... x... ....", Hat b"X.x. x.x. X.x. x...")),
    phrase!(0, "Trap", "Open-hat relay", 149, 0.0, "Open hats move across the phrase before a low-tom and hat turnaround.";
        bar!("Open-hat groove"; Kick b"X... .x.. .... ..x.", Clap b".... .... X... ....", Hat b"x.x. x... X.x. x.x.", OpenHat b".... ..x. .... ...."),
        bar!("Open-hat answer"; Kick b"X... ...x .x.. ....", Clap b".... .... X... ....", Hat b"x.x. x.x. X... ..x.", OpenHat b".... .... ..x. ...."),
        bar!("Broken pocket"; Kick b"X..x .... .... ...x", Clap b".... .... X... ....", Hat b"x... ..x. x.g. ..2.", OpenHat b".... .... .... x..."),
        bar!("Tom and hat fill"; Kick b"X... ..x. .... ....", Clap b".... .... X... ....", Hat b"x.x. x.x. x... .x24", LowTom b".... .... ...g .xX.")),
    beat!(0, "Drill", "Sliding pocket", 142, 0.0, "An uneven hat pulse and a late second snare leave gaps for sliding bass.";
        Kick b"X... .x.. ...x .x..", Snare b".... .... X... ..x.", Hat b"X..x ..x. X..x ..2.", Rim b".... ...g .... ...."),
    beat!(0, "R&B", "Neo-soul pocket", 83, 0.25, "A warm backbeat with ghost notes and lightly swung hats.";
        Kick b"X..g ..x. x... ...x", Snare b"...g X... ..g. X...", Hat b"X.g. x..g X.g. x..."),
    beat!(0, "Breakbeat hip-hop", "Dusty break", 103, 0.10, "A broken kick phrase with a ghost snare before the turnaround.";
        Kick b"X..x .... X.x. ..x.", Snare b".... X... ...g X..g", Hat b"X.x. ..x. .x.x ..x."),
    beat!(0, "Lo-fi hip-hop", "Soft ghost pocket", 74, 0.36, "Quiet hats and ghost snares sit around a sparse kick.";
        Kick b"X... ..x. .... ...x", Snare b"...g x... ..g. x...", Hat b"x..g ..g. x..g ..g."),
    beat!(0, "Lo-fi hip-hop", "Tape sway", 81, 0.42, "A snare pushed to the end of beat four adds a loose turn.";
        Kick b"X... .x.. ..x. ....", Snare b".... x... ...g ...X", Hat b"x..g x..g x..g x..."),
    beat!(0, "Lo-fi hip-hop", "Brushed ride", 77, 0.30, "Soft ride notes fill the spaces left by a minimal rim backbeat.";
        Kick b"x... .... ..x. ....", Rim b".... x... .... x...", Ride b"x.g. .g.. x.g. .g.."),
    beat!(0, "Jazz rap", "Jazz club pocket", 90, 0.18, "Ride accents, ghost snares and a low kick suggest a small live kit.";
        Kick b"X... ...g ..x. ....", Snare b"...g x... .g.. x...", Ride b"X..x x... X..x x..g"),
    beat!(0, "Jazz rap", "Ride and rim", 96, 0.25, "Rim accents and a loose ride leave room for melodic chops.";
        Kick b"X... ..x. .x.. ....", Rim b".... X... ...g X...", Ride b"X.x. ..g. x..x ..g."),
    beat!(0, "Drill", "Broken backbeat", 144, 0.0, "Staggered snares and short hat rolls sit over an uneven kick.";
        Kick b"X... ..x. ...x ....", Snare b".... .... .X.. ...x", Hat b"X..x ..g. X..x .2.."),
    beat!(0, "Drill", "Empty downbeat", 140, 0.0, "The first kick enters late, with a hard clap on three.";
        Kick b"...X .... ..x. ..x.", Clap b".... .... X... ..x.", Hat b"x..x ..x. x..x ..3."),
    beat!(0, "Drill", "Double snare answer", 146, 0.0, "Two late snares answer a single strong hit on three.";
        Kick b"X... .x.. .... ...x", Snare b".... .... X... .xx.", Hat b"X..g ..x. X..g ..2."),
    beat!(0, "Memphis rap", "Memphis machine", 118, 0.04, "A tight machine backbeat with dry rim punctuation and quick hats.";
        Kick b"X... ...x X..x ....", Snare b".... X... .... X...", Rim b".... ..g. .... ..x.", Hat b"X.x. x.x2 X.x. x.x."),
    beat!(0, "Memphis rap", "Memphis half-time", 130, 0.0, "A sparse half-time clap with an insistent rim phrase.";
        Kick b"X... ..x. .x.. ..x.", Clap b".... .... X... ....", Rim b"..x. ...x ..x. x...", Hat b"x.x. x.x. ..x. x.x."),
    beat!(0, "West Coast hip-hop", "G-funk pocket", 94, 0.13, "A relaxed clap and open hats make room for a syncopated bass part.";
        Kick b"X... .x.. X.x. ....", Clap b".... X... .... X...", Hat b"x.x. x... x.x. x...", OpenHat b".... ..x. .... ..x."),
    beat!(0, "West Coast hip-hop", "Bay bounce", 102, 0.04, "Short kick doubles and a bright clap drive the bounce.";
        Kick b"Xx.. .... X..x ..x.", Clap b".... X... .... X...", Hat b"x... ..X. x... ..X.", Rim b".... ...g .... ...g"),
    beat!(0, "Southern hip-hop", "Southern bounce", 98, 0.08, "Clipped hats and a low-tom answer accent the kick doubles.";
        Kick b"X..x ..x. X... .x..", Clap b".... X... .... X...", Hat b"x.x. ..x. x.x. ..x.", LowTom b".... .... ..g. ...x"),
    beat!(0, "Southern hip-hop", "Crunk stomp", 150, 0.0, "Heavy quarter-note accents and a layered half-time clap.";
        Kick b"X... x... X... x.x.", Clap b".... .... X... ....", Snare b".... .... x... ....", Hat b"..x. ..x. ..x. ..x.", Crash b"X... .... .... ...."),
    beat!(0, "Breakbeat hip-hop", "Chopped funk break", 107, 0.10, "Ghost snares and quick kick pickups give the bar a chopped-break feel.";
        Kick b"X..x ..g. X.x. ...x", Snare b"..g. X..g .g.. X.g.", Hat b"X.x. x... X.x. x.x."),
    beat!(0, "Experimental hip-hop", "Lopsided break", 88, 0.33, "An uneven snare placement and scattered hats make a deliberately crooked loop.";
        Kick b"X... .x.. ...x ..x.", Snare b".... .X.. ..g. X...", Hat b"X..g .x.g ..x. .g.x"),
    beat!(1, "House", "Four on the floor", 124, 0.0, "Quarter-note kick, offbeat open hats and claps on two and four.";
        Kick b"X... X... X... X...", Clap b".... X... .... X...", Hat b".g.. .g.. .g.. .g..", OpenHat b"..x. ..x. ..x. ..x."),
    beat!(1, "Deep house", "Deep shuffle", 119, 0.23, "Four steady kicks under soft, shuffling percussion.";
        Kick b"X... X... X... X...", Clap b".... X... .... X...", Hat b".g.x .g.x .g.x .g.x", OpenHat b"..x. .... ..x. ....", Rim b".... ...g .x.. ...g"),
    beat!(1, "Techno", "Driving techno", 136, 0.0, "A straight kick with shifting hats and a low-tom pulse.";
        Kick b"X... X... X... X...", Clap b".... x... .... x...", Hat b".g.x .x.g .g.x .x.g", OpenHat b"..x. ..x. ..x. ..X.", LowTom b".... ...x .... .x.."),
    beat!(1, "Electro", "Electro", 127, 0.0, "Machine-like hats and a broken kick under a crisp backbeat.";
        Kick b"X..x .... X..x ....", Snare b".... X... .... X..g", Hat b"..x. ..x. ..x. ..x."),
    beat!(1, "Disco", "Disco offbeats", 116, 0.0, "A steady kick with alternating closed and open hats.";
        Kick b"X... x... X... x...", Snare b".... X... .... X...", Hat b"x... x... x... x...", OpenHat b"..x. ..x. ..x. ..x."),
    beat!(1, "UK garage", "Two-step garage", 132, 0.30, "Two broken kicks, clipped hats and a swung backbeat.";
        Kick b"X... .... ..x. ....", Clap b".... X... .... X...", Hat b"..xg .g.x ..xg .g.x", OpenHat b".... ..x. .... ..x.", Rim b".... ...g .g.. ...."),
    beat!(1, "Drum & bass", "Two-step drum & bass", 174, 0.0, "The second kick lands between backbeats, with quick hats above.";
        Kick b"X... .... ..x. ....", Snare b".... X... .... X...", Hat b"X.x. x.x. X.x. x.xg", OpenHat b".... .... .... ..x."),
    beat!(1, "Jungle", "Jungle break", 166, 0.08, "An accented ride and ghost snares carry a busy broken beat.";
        Kick b"X... ..x. ..x. ...x", Snare b"...g X... .g.g X..g", Ride b"X.x. x.x. X.x. x.x.", Hat b".... ...x .... ...x"),
    beat!(1, "Dubstep", "Half-time dubstep", 140, 0.0, "A spacious kick and a strong snare on beat three.";
        Kick b"X... ..x. .... ..x.", Snare b".... .... X... ....", Hat b"..x. ..g. ..x. ..2.", OpenHat b".... .... .... x..."),
    beat!(2, "Rock", "Straight rock", 112, 0.0, "An eighth-note hat and a clear two-and-four backbeat.";
        Kick b"X... .... X.x. ....", Snare b".... X... .... X...", Hat b"X.x. x.x. X.x. x.x."),
    beat!(2, "Punk", "Fast punk", 185, 0.0, "Fast eighths with a punchy backbeat and a crash on one.";
        Kick b"X.x. .... X.x. ....", Snare b".... X... .... X...", Hat b"x.x. x.x. x.x. x.x.", Crash b"X... .... .... ...."),
    beat!(2, "Metal", "Double-kick drive", 162, 0.0, "Sixteenth-note double kick with a cymbal pulse and strong backbeat.";
        Kick b"Xxxx Xxxx Xxxx Xxxx", Snare b".... X... .... X...", Ride b"X.x. x.x. X.x. x.x.", Crash b"X... .... .... ...."),
    beat!(2, "Pop", "Pop backbeat", 108, 0.0, "An even pop pulse with a layered clap and open-hat lift.";
        Kick b"X... ..x. X... ..x.", Snare b".... X... .... X...", Clap b".... g... .... x...", Hat b"x.x. x.x. x.x. x...", OpenHat b".... .... .... ..x."),
    beat!(2, "Funk", "Syncopated funk", 106, 0.12, "Syncopated kicks, ghost snares and accented sixteenth-note hats.";
        Kick b"X..x ..x. .x.x ..x.", Snare b"...g X..g ..g. X..g", Hat b"Xgxg xgxg Xgxg xgxg"),
    beat!(2, "Jazz", "Jazz ride", 136, 0.12, "Ride accents with a late skip, light kick and quiet snare comping.";
        Kick b"g... g... g... g...", Snare b".... ...g .... .g..", Hat b".... x... .... x...", Ride b"X... x..x X... x..x"),
    beat!(2, "Blues", "Blues shuffle", 100, 0.0, "A shuffle-inspired ride on the sixteenth-note grid with a steady backbeat.";
        Kick b"X... ..x. X... ....", Snare b".... X... .... X...", Ride b"X..x x..x X..x x..x"),
    beat!(2, "Soul", "Pocket soul", 96, 0.12, "A relaxed eighth-note hat with a kick pickup into the backbeat.";
        Kick b"X... ..x. X... ...x", Snare b".... X... ..g. X...", Hat b"X.x. x.x. X.x. x.x."),
    beat!(3, "Reggaeton", "Dembow", 96, 0.0, "A kick on one and three with an offbeat snare and answering backbeat.";
        Kick b"X... .... X... ....", Snare b"...x X... ...x X...", Hat b"x.x. x.x. x.x. x.x."),
    beat!(3, "Reggae", "One drop", 76, 0.12, "Kick and rim land together on beat three under light offbeats.";
        Kick b".... .... X... ....", Rim b".... .... X... ....", Hat b"x.x. x.x. x.x. x...", OpenHat b".... .... .... ..x."),
    beat!(3, "Dancehall", "Digital dancehall", 102, 0.0, "An angular kick and rim pattern with space between accents.";
        Kick b"X... ..x. .... ..x.", Rim b"...x .... X... ..x.", Hat b"..x. .g.. ..x. .g..", Clap b".... .... g... ...."),
    beat!(3, "Afrobeats", "Afrobeats pocket", 106, 0.13, "A bouncing kick and interlocking rim and hat accents.";
        Kick b"X... ..x. .x.. ..x.", Rim b"...x .... x..x ....", Hat b"x.g. .x.g x.g. .x.g", OpenHat b".... .... ..x. ...."),
    beat!(3, "Amapiano", "Amapiano percussion", 112, 0.10, "A light kit groove with a low-tom pattern that suggests a log drum.";
        Kick b"X... x... X... x...", Rim b".... x... .... x...", Hat b".g.x .g.x .g.x .g.x", LowTom b".... ..x. .x.. ..x."),
    beat!(3, "Latin pop", "Clave-inspired pop", 108, 0.0, "A rim pattern inspired by clave over a light pop kick and hats.";
        Kick b"X... ..x. X... ....", Rim b"X..x ..x. ..x. x...", Hat b"x.x. x.x. x.x. x.x.", LowTom b".... ...g .... ..x."),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DRUMS, Studio};

    #[test]
    fn templates_are_well_formed_and_applying_one_replaces_only_drum_steps() {
        let mut names = std::collections::HashSet::new();
        let mut rhythms = std::collections::HashSet::new();
        for template in TEMPLATES {
            assert!(template.family < FAMILIES.len());
            assert!(
                names.insert(template.name),
                "duplicate name {}",
                template.name
            );
            assert!(matches!(template.bars.len(), 1 | 2 | 4));
            let mut phrase = Vec::new();
            for template_bar in template.bars {
                let mut seen = [false; DRUMS.len()];
                for &(lane, pattern) in template_bar.lanes {
                    assert!(!std::mem::replace(&mut seen[lane as usize], true));
                    assert!([4, 9, 14].into_iter().all(|i| pattern[i] == b' '));
                }
                let bar = template_bar.steps();
                assert!(bar.drums.iter().flatten().any(|&hit| hit > 0));
                // Velocity alone does not make two bars rhythmically distinct.
                let rhythm: [[u8; STEPS]; DRUMS.len()] = std::array::from_fn(|lane| {
                    std::array::from_fn(|step| {
                        let cell = ((lane + 2) * STEPS + step) as u8;
                        bar.rules
                            .get(&cell)
                            .map_or(u8::from(bar.drums[lane][step] > 0), |rule| rule.repeats)
                    })
                });
                assert!(
                    !phrase.contains(&rhythm),
                    "repeated bar in {}",
                    template.name
                );
                phrase.push(rhythm);
            }
            assert!(rhythms.insert(phrase), "duplicate rhythm {}", template.name);
        }

        let mut studio = Studio {
            bars: 2,
            ..Studio::default()
        };
        let old_rule = StepRule {
            chance: 50,
            ..StepRule::default()
        };
        studio.set_step_rule(1, 0, 0, old_rule);
        studio.set_step_rule(1, 2, 0, old_rule);
        studio.chops[1][0] = Some(2);
        studio.hits[0][0][0] = 90;
        let trap = TEMPLATES
            .iter()
            .find(|template| template.genre == "Trap")
            .unwrap();
        assert!(studio.apply_drum_template(1, trap, false));
        let bar = studio.bar_notes(1);
        assert_eq!(bar.drums, trap.bars[0].steps().drums);
        assert_eq!(bar.pads[0], Some(2));
        assert_eq!(bar.rules[&0], old_rule);
        assert!(!bar.rules.contains_key(&((2 * STEPS) as u8)));
        assert!(bar.rules.values().any(|rule| rule.repeats == 3));
        assert_eq!(studio.hits[0][0][0], 90);
        assert_eq!(studio.bpm, Studio::default().bpm);
    }

    #[test]
    fn applying_a_phrase_extends_the_pattern_and_preserves_non_drum_steps() {
        let phrase = TEMPLATES
            .iter()
            .find(|template| template.genre == "Trap" && template.bars.len() == 4)
            .unwrap();
        let mut studio = Studio {
            bars: 3,
            ..Studio::default()
        };
        studio.hits[0][0][1] = 60;
        studio.chops[1][4] = Some(3);
        studio.key_steps[2][6] = Some(60);
        let rule = StepRule {
            chance: 60,
            ..StepRule::default()
        };
        studio.set_step_rule(2, 1, 6, rule);

        assert!(studio.apply_drum_template(1, phrase, true));
        assert_eq!(studio.bars, 5);
        assert_eq!(studio.hits[0][0][1], 60);
        assert_eq!(studio.chops[1][4], Some(3));
        assert_eq!(studio.key_steps[2][6], Some(60));
        assert_eq!(studio.step_rules[2][&((STEPS + 6) as u8)], rule);
        assert_eq!(studio.bpm, phrase.bpm);
        for (bar, template_bar) in (1..5).zip(phrase.bars) {
            assert_eq!(studio.hits[bar], template_bar.steps().drums);
        }
        assert!(studio.step_rules[4].values().any(|rule| rule.repeats == 4));

        studio.bars = MAX_BARS;
        let before = studio.clone();
        assert!(!studio.apply_drum_template(MAX_BARS - 2, phrase, true));
        assert_eq!(studio, before);
    }
}
