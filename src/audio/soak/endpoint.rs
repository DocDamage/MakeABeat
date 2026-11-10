use super::PILOT_LEVEL;
use std::collections::BTreeMap;
use windows::Win32::{
    Media::Audio::{
        AudioSessionStateActive, Endpoints::IAudioMeterInformation, IAudioSessionControl2,
        IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
        eConsole, eRender,
    },
    System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
    },
};
use windows::core::{Interface, Result};

struct Apartment;

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub(super) struct Endpoint {
    manager: IAudioSessionManager2,
    peaks: BTreeMap<u32, f32>,
    _apartment: Apartment,
}

impl Endpoint {
    pub(super) fn open() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            let apartment = Apartment;
            let devices: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            // CPAL's default WASAPI device uses the console role.
            let device = devices.GetDefaultAudioEndpoint(eRender, eConsole)?;
            Ok(Self {
                manager: device.Activate(CLSCTX_ALL, None)?,
                peaks: BTreeMap::new(),
                _apartment: apartment,
            })
        }
    }

    pub(super) fn sample_other_audio(&mut self) -> Result<()> {
        unsafe {
            let sessions = self.manager.GetSessionEnumerator()?;
            for index in 0..sessions.GetCount()? {
                let session = sessions.GetSession(index)?;
                if session.GetState()? != AudioSessionStateActive {
                    continue;
                }
                let process = session.cast::<IAudioSessionControl2>()?.GetProcessId()?;
                if process == std::process::id() {
                    continue;
                }
                let volume = session.cast::<ISimpleAudioVolume>()?;
                if volume.GetMute()?.as_bool() {
                    continue;
                }
                let peak = session.cast::<IAudioMeterInformation>()?.GetPeakValue()?
                    * volume.GetMasterVolume()?;
                if peak > PILOT_LEVEL / 100.0 {
                    let previous = self.peaks.entry(process).or_insert_with(|| {
                        println!("Competing endpoint audio from PID {process}");
                        0.0
                    });
                    *previous = previous.max(peak);
                }
            }
        }
        Ok(())
    }

    pub(super) fn require_quiet(&self) -> crate::files::Result<()> {
        if self.peaks.is_empty() {
            return Ok(());
        }
        Err(format!(
            "Endpoint soak inconclusive: other audio played on the default output (peak by PID {:?}). Pause other audio and rerun.",
            self.peaks
        )
        .into())
    }
}
