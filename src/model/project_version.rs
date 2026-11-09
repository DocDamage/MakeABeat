#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProjectVersion {
    SingleSource,
    MultipleSources,
    ChokeGroups,
    Snapshots,
    Instruments,
    BeatMaps,
    Grooves,
    Transcription,
    Extraction,
}

impl ProjectVersion {
    pub fn read(marker: [u8; 8]) -> Result<Self, &'static str> {
        match &marker {
            b"LSFLIP01" => Ok(Self::SingleSource),
            b"LSFLIP02" => Ok(Self::MultipleSources),
            b"LSFLIP03" => Ok(Self::ChokeGroups),
            b"LSFLIP04" => Ok(Self::Snapshots),
            b"LSFLIP05" => Ok(Self::Instruments),
            b"LSFLIP06" => Ok(Self::BeatMaps),
            b"LSFLIP07" => Ok(Self::Grooves),
            b"LSFLIP08" => Ok(Self::Transcription),
            b"LSFLIP09" => Ok(Self::Extraction),
            _ if marker.starts_with(b"LSFLIP") => {
                Err("Unsupported Lost Samples project format. Open it in a compatible version.")
            }
            _ => Err("This file isn't a Lost Samples project."),
        }
    }

    pub fn marker(self) -> &'static [u8; 8] {
        match self {
            Self::SingleSource => b"LSFLIP01",
            Self::MultipleSources => b"LSFLIP02",
            Self::ChokeGroups => b"LSFLIP03",
            Self::Snapshots => b"LSFLIP04",
            Self::Instruments => b"LSFLIP05",
            Self::BeatMaps => b"LSFLIP06",
            Self::Grooves => b"LSFLIP07",
            Self::Transcription => b"LSFLIP08",
            Self::Extraction => b"LSFLIP09",
        }
    }

    pub fn accepts_sources(self, extras: usize) -> bool {
        extras < super::MAX_SOURCES
            && match self {
                Self::SingleSource => extras == 0,
                Self::MultipleSources => extras > 0,
                _ => true,
            }
    }
}
