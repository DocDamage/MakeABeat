use crate::audio::Command;
use eframe::egui;
use midir::{Ignore, MidiInput, MidiInputConnection};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    time::Duration,
};

const CLIENT: &str = "Lost Samples Desktop";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Message {
    Note(u8, u8, u8),
    Control(u8, u8, u8),
    Bend(u8, u16),
    Tick,
    Start,
    Continue,
    Stop,
    Position(u16),
}

impl Message {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        match bytes {
            [0xf8] => Some(Self::Tick),
            [0xfa] => Some(Self::Start),
            [0xfb] => Some(Self::Continue),
            [0xfc] => Some(Self::Stop),
            [status, a, b] if *a < 128 && *b < 128 => {
                let channel = status & 15;
                match status & 0xf0 {
                    0x80 => Some(Self::Note(channel, *a, 0)),
                    0x90 => Some(Self::Note(channel, *a, *b)),
                    0xb0 => Some(Self::Control(channel, *a, *b)),
                    0xe0 => Some(Self::Bend(channel, u16::from(*a) | (u16::from(*b) << 7))),
                    0xf0 if *status == 0xf2 => {
                        Some(Self::Position(u16::from(*a) | (u16::from(*b) << 7)))
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub fn signal(self) -> Option<(Signal, u8)> {
        match self {
            Self::Note(channel, number, velocity) => {
                Some((Signal::Note(channel, number), velocity))
            }
            Self::Control(channel, number, value) => Some((Signal::Cc(channel, number), value)),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Signal {
    Note(u8, u8),
    Cc(u8, u8),
}

impl Signal {
    pub fn label(self) -> String {
        match self {
            Self::Note(ch, n) => format!("Ch {} · Note {n}", ch + 1),
            Self::Cc(ch, n) => format!("Ch {} · CC {n}", ch + 1),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Target {
    Play,
    Stop,
    Record,
    Loop,
    Preview,
    Tap,
    SampleLevel,
    DrumLevel,
    Filter,
    Delay,
    Reverb,
    Swing,
    ChopGain,
    ChopPan,
    ChopTune,
    ChopAttack,
    ChopRelease,
    ChopReverse,
    Pad(usize),
    Drum(usize),
}

impl Target {
    pub const FIXED: [Self; 18] = [
        Self::Play,
        Self::Stop,
        Self::Record,
        Self::Loop,
        Self::Preview,
        Self::Tap,
        Self::SampleLevel,
        Self::DrumLevel,
        Self::Filter,
        Self::Delay,
        Self::Reverb,
        Self::Swing,
        Self::ChopGain,
        Self::ChopPan,
        Self::ChopTune,
        Self::ChopAttack,
        Self::ChopRelease,
        Self::ChopReverse,
    ];

    pub fn label(self) -> String {
        match self {
            Self::Play => "Play",
            Self::Stop => "Stop",
            Self::Record => "Toggle recording",
            Self::Loop => "Toggle loop",
            Self::Preview => "Toggle preview",
            Self::Tap => "Tap tempo",
            Self::SampleLevel => "Sample level",
            Self::DrumLevel => "Drum level",
            Self::Filter => "Filter",
            Self::Delay => "Delay",
            Self::Reverb => "Reverb",
            Self::Swing => "Swing",
            Self::ChopGain => "Selected chop gain",
            Self::ChopPan => "Selected chop pan",
            Self::ChopTune => "Selected chop tune",
            Self::ChopAttack => "Selected chop attack",
            Self::ChopRelease => "Selected chop release",
            Self::ChopReverse => "Selected chop reverse",
            Self::Pad(index) => return format!("Chop pad {}", index + 1),
            Self::Drum(index) => {
                return crate::model::DRUMS
                    .get(index)
                    .unwrap_or(&"Drum")
                    .to_string();
            }
        }
        .into()
    }

    pub fn continuous(self) -> bool {
        matches!(
            self,
            Self::SampleLevel
                | Self::DrumLevel
                | Self::Filter
                | Self::Delay
                | Self::Reverb
                | Self::Swing
                | Self::ChopGain
                | Self::ChopPan
                | Self::ChopTune
                | Self::ChopAttack
                | Self::ChopRelease
        )
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Route {
    #[default]
    Auto,
    Piano,
    Pads,
    Controls,
}

impl Route {
    pub const ALL: [Self; 4] = [Self::Auto, Self::Piano, Self::Pads, Self::Controls];

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Follow Studio view",
            Self::Piano => "Piano keys",
            Self::Pads => "Chop pads",
            Self::Controls => "Controls only",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VelocityCurve {
    #[default]
    Linear,
    Soft,
    Hard,
    Fixed,
}

impl VelocityCurve {
    pub const ALL: [Self; 4] = [Self::Linear, Self::Soft, Self::Hard, Self::Fixed];
    pub fn label(self) -> &'static str {
        match self { Self::Linear => "Linear", Self::Soft => "Soft", Self::Hard => "Hard", Self::Fixed => "Fixed 100" }
    }
    pub fn apply(self, value: u8) -> u8 {
        if value == 0 { return 0; }
        let unit = f32::from(value) / 127.0;
        let curved = match self {
            Self::Linear => return value,
            Self::Soft => unit.sqrt(),
            Self::Hard => unit * unit,
            Self::Fixed => return 100,
        };
        (curved * 127.0).round().clamp(1.0, 127.0) as u8
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CcMode {
    #[default]
    Absolute,
    RelativeTwosComplement,
    RelativeBinaryOffset,
}

impl CcMode {
    pub const ALL: [Self; 3] = [Self::Absolute, Self::RelativeTwosComplement, Self::RelativeBinaryOffset];
    pub fn label(self) -> &'static str {
        match self {
            Self::Absolute => "Absolute",
            Self::RelativeTwosComplement => "Relative · two's complement",
            Self::RelativeBinaryOffset => "Relative · binary offset",
        }
    }
    pub fn delta(self, value: u8) -> Option<i16> {
        let value = i16::from(value);
        match self {
            Self::Absolute => None,
            Self::RelativeTwosComplement => Some(if value >= 64 { value - 128 } else { value }),
            Self::RelativeBinaryOffset => Some(value - 64),
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PortSettings {
    pub disabled: bool,
    pub route: Route,
    pub bindings: Vec<(Signal, Target)>,
    pub channel: Option<u8>,
    pub velocity: VelocityCurve,
    pub cc_mode: CcMode,
    pub soft_takeover: bool,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub ports: HashMap<String, PortSettings>,
    pub clock: Option<String>,
    pub piano_view: bool,
    pub output: Option<String>,
    pub send_clock: bool,
}

impl Settings {
    pub fn is_empty(&self) -> bool {
        self.ports.is_empty() && self.clock.is_none() && !self.piano_view
            && self.output.is_none() && !self.send_clock
    }

    pub fn disabled(&self) -> HashSet<String> {
        self.ports
            .iter()
            .filter(|(_, settings)| settings.disabled)
            .map(|(id, _)| id.clone())
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Generic,
    LaunchkeyMk4,
}

impl Profile {
    fn for_name(name: &str) -> Self {
        let name: String = name
            .to_ascii_lowercase()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect();
        if (name.contains("launchkey") && name.contains("mk4") || name.contains("lkmk4"))
            && !name.contains("daw")
            && !name.contains("midiin2")
        {
            Self::LaunchkeyMk4
        } else {
            Self::Generic
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Generic => "Standard MIDI",
            Self::LaunchkeyMk4 => "Novation Launchkey MK4 · standalone",
        }
    }

    pub fn pad(self, signal: Signal) -> Option<usize> {
        let Signal::Note(9, note) = signal else {
            return None;
        };
        if self == Self::LaunchkeyMk4 {
            // Number the bottom row first, followed by the top row.
            [
                36, 37, 38, 39, 44, 45, 46, 47, 40, 41, 42, 43, 48, 49, 50, 51,
            ]
            .iter()
            .position(|&value| value == note)
        } else {
            (36..52).contains(&note).then(|| usize::from(note - 36))
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub profile: Profile,
    pub connection: Option<u64>,
    pub error: Option<String>,
}

pub enum Event {
    Devices(Vec<Device>),
    Message(u64, Message),
    Clock(u64, Message, Option<f64>),
    Error(String),
}

pub struct Input {
    events: Receiver<Event>,
    configure: Sender<HashSet<String>>,
    clock: Arc<Mutex<ClockLink>>,
}

#[derive(Default)]
struct Clock {
    last_tick: Option<u64>,
    period: Option<f64>,
}

impl Clock {
    fn pulse(&mut self, timestamp: u64) -> Option<f64> {
        if let Some(last) = self.last_tick.replace(timestamp) {
            let seconds = timestamp.saturating_sub(last) as f64 / 1_000_000.0;
            if (0.008..=0.080).contains(&seconds) {
                self.period = Some(
                    self.period
                        .map_or(seconds, |old| old * 0.85 + seconds * 0.15),
                );
            } else if seconds > 0.5 {
                self.period = None;
            }
        }
        self.period
    }
}

#[derive(Default)]
struct ClockLink {
    port: Option<u64>,
    output: Option<Sender<Command>>,
    timing: Clock,
    fallback_period: f64,
}

impl ClockLink {
    fn event(&mut self, connection: u64, timestamp: u64, message: Message) -> Option<Event> {
        if self.port != Some(connection) {
            return (message != Message::Tick).then_some(Event::Message(connection, message));
        }
        let mut period = None;
        let command = match message {
            Message::Tick => {
                period = self.timing.pulse(timestamp);
                Command::ClockPulse(period.unwrap_or(self.fallback_period))
            }
            Message::Start => Command::ClockStart(false),
            Message::Continue => Command::ClockStart(true),
            Message::Stop => Command::Pattern(false),
            Message::Position(step) => Command::ClockPosition(step),
            _ => return Some(Event::Message(connection, message)),
        };
        if let Some(output) = &self.output {
            let _ = output.send(command);
        }
        Some(Event::Clock(connection, message, period))
    }
}

impl Input {
    pub fn start(ctx: egui::Context, disabled: HashSet<String>) -> Self {
        let (events_tx, events) = mpsc::channel();
        let (configure, settings) = mpsc::channel();
        let clock = Arc::new(Mutex::new(ClockLink::default()));
        let worker_clock = Arc::clone(&clock);
        std::thread::spawn(move || monitor(&ctx, &events_tx, settings, disabled, &worker_clock));
        Self {
            events,
            configure,
            clock,
        }
    }

    pub fn event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    pub fn configure(&self, settings: &Settings) {
        let _ = self.configure.send(settings.disabled());
    }

    pub fn route_clock(&self, port: Option<u64>, output: Option<Sender<Command>>, bpm: u16) {
        let Ok(mut clock) = self.clock.lock() else {
            return;
        };
        if let Some(output) = &output {
            let _ = output.send(Command::ClockMode(port.is_some()));
        }
        *clock = ClockLink {
            port,
            output,
            timing: Clock::default(),
            fallback_period: 60.0 / (24.0 * f64::from(bpm)),
        };
    }
}

fn monitor(
    ctx: &egui::Context,
    events: &Sender<Event>,
    settings: Receiver<HashSet<String>>,
    mut disabled: HashSet<String>,
    clock: &Arc<Mutex<ClockLink>>,
) {
    let Ok(input) = MidiInput::new(CLIENT) else {
        let _ = events.send(Event::Error(
            "Windows MIDI input could not be initialized.".into(),
        ));
        ctx.request_repaint();
        return;
    };
    let mut open: HashMap<String, (u64, MidiInputConnection<()>)> = HashMap::new();
    let mut previous: Vec<Device> = Vec::new();
    let mut next_connection = 1;
    loop {
        let mut devices = Vec::new();
        let ports = input.ports();
        let ids: HashSet<_> = ports.iter().map(|port| port.id()).collect();
        open.retain(|id, _| ids.contains(id) && !disabled.contains(id));
        for port in ports {
            let id = port.id();
            let Ok(name) = input.port_name(&port) else {
                continue;
            };
            let mut device = Device {
                profile: Profile::for_name(&name),
                name,
                connection: None,
                error: None,
                id: id.clone(),
            };
            if !disabled.contains(&id) && !open.contains_key(&id) {
                let connection = next_connection;
                let ctx = ctx.clone();
                let events = events.clone();
                let clock = Arc::clone(clock);
                let result = MidiInput::new(CLIENT)
                    .map_err(|error| error.to_string())
                    .and_then(|mut input| {
                        input.ignore(Ignore::SysexAndActiveSense);
                        input
                            .connect(
                                &port,
                                CLIENT,
                                move |timestamp, bytes, _| {
                                    if let Some(message) = Message::parse(bytes)
                                        && let Some(event) =
                                            clock.lock().ok().and_then(|mut clock| {
                                                clock.event(connection, timestamp, message)
                                            })
                                        && events.send(event).is_ok()
                                    {
                                        ctx.request_repaint();
                                    }
                                },
                                (),
                            )
                            .map_err(|error| error.to_string())
                    });
                match result {
                    Ok(input) => {
                        next_connection += 1;
                        open.insert(id.clone(), (connection, input));
                    }
                    Err(error) => device.error = Some(error),
                }
            }
            device.connection = open.get(&id).map(|(connection, _)| *connection);
            devices.push(device);
        }
        devices.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        if devices != previous {
            previous = devices.clone();
            if events.send(Event::Devices(devices)).is_err() {
                return;
            }
            ctx.request_repaint();
        }
        match settings.recv_timeout(Duration::from_secs(2)) {
            Ok(new) => disabled = new,
            Err(mpsc::RecvTimeoutError::Timeout) => (),
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore = "requires a Windows MIDI loopback named Lost Samples MIDI QA"]
    fn windows_port_reports_its_name_delivers_messages_and_disconnects() {
        let input = Input::start(egui::Context::default(), HashSet::new());
        let (audio, commands) = mpsc::channel();
        let mut connection = None;
        let mut got_note = false;
        let mut got_control = false;
        let mut disconnected = false;
        let deadline = Instant::now() + Duration::from_secs(22);
        while Instant::now() < deadline && !disconnected {
            match input.event() {
                Some(Event::Devices(devices)) => {
                    let port = devices
                        .iter()
                        .find(|device| device.name == "Lost Samples MIDI QA");
                    if let Some(device) = port {
                        assert!(!device.id.is_empty());
                        if connection.is_none() {
                            connection = device.connection;
                            input.route_clock(connection, Some(audio.clone()), 120);
                        }
                    } else {
                        disconnected = connection.is_some();
                    }
                }
                Some(Event::Message(id, Message::Note(0, 108, 105))) => {
                    got_note |= connection == Some(id);
                }
                Some(Event::Message(id, Message::Control(0, 7, 96))) => {
                    got_control |= connection == Some(id);
                }
                Some(Event::Error(error)) => panic!("{error}"),
                Some(_) => (),
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
        assert!(got_note && got_control && disconnected);
        let commands: Vec<_> = commands.try_iter().collect();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::ClockMode(true)))
        );
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::ClockStart(false)))
        );
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, Command::ClockPulse(_)))
        );
    }
}
