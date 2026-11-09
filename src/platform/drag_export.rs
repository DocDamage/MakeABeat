use crate::{
    audio::Clip,
    files::{self, WavOptions},
    model::Studio,
};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::Arc,
};
use windows::{
    Win32::{
        Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK},
        System::{
            Com::IDataObject,
            Ole::{
                DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DoDragDrop, IDropSource,
                IDropSource_Impl, OleInitialize, OleUninitialize,
            },
            SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS},
        },
        UI::Shell::{BHID_DataObject, IShellItem, SHCreateItemFromParsingName},
    },
    core::{BOOL, HRESULT, PCWSTR, implement},
};

struct Apartment;

impl Apartment {
    fn enter() -> windows::core::Result<Self> {
        // The UI thread owns the OLE apartment for the lifetime of the drag.
        unsafe { OleInitialize(None)? };
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe { OleUninitialize() };
    }
}

#[implement(IDropSource)]
struct Source;

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if !keys.contains(MK_LBUTTON) {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

fn data_object(path: &Path) -> windows::core::Result<IDataObject> {
    let name: Vec<_> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    // The Shell provides standard file formats including CF_HDROP.
    unsafe {
        let file: IShellItem = SHCreateItemFromParsingName(PCWSTR(name.as_ptr()), None)?;
        file.BindToHandler(None, &BHID_DataObject)
    }
}

pub fn drag(path: &Path) -> windows::core::Result<bool> {
    let _apartment = Apartment::enter()?;
    let object = data_object(path)?;
    let source: IDropSource = Source.into();
    let mut effect = DROPEFFECT(0);
    let result = unsafe {
        DoDragDrop(
            &object,
            &source,
            DROPEFFECT_COPY | DROPEFFECT_LINK,
            &mut effect,
        )
    };
    result.ok()?;
    Ok(result == DRAGDROP_S_DROP && effect.0 != 0)
}

#[derive(Clone, Copy)]
pub enum Export {
    Selection { source: usize, range: (f64, f64) },
    Pad(usize),
    Pattern,
}

pub fn directory() -> files::Result<PathBuf> {
    let path = files::app_data_dir()
        .ok_or("No app data directory is available.")?
        .join("drag-exports");
    std::fs::create_dir_all(&path)?;
    Ok(path)
}

pub fn prepare(
    directory: &Path,
    export: Export,
    mut studio: Studio,
    clip: Option<Arc<Clip>>,
    wav: WavOptions,
) -> files::Result<PathBuf> {
    let mut hash = DefaultHasher::new();
    "lost-samples-drag-v1".hash(&mut hash);
    wav.bits.hash(&mut hash);
    wav.rate.hash(&mut hash);
    let (name, selected) = match export {
        Export::Selection { source, range } => {
            let audio = clip
                .as_deref()
                .and_then(|clip| clip.source(source))
                .ok_or("The selected audio is unavailable.")?;
            let frames = audio
                .region(range)
                .ok_or("Select a non-empty audio range.")?;
            range.0.to_bits().hash(&mut hash);
            range.1.to_bits().hash(&mut hash);
            hash_audio(&mut hash, audio.rate, frames)?;
            ("Cut".to_owned(), Some((audio, range)))
        }
        Export::Pad(pad) => (format!("Pad {:02}", pad + 1), None),
        Export::Pattern => ("Pattern".to_owned(), None),
    };
    if selected.is_none() {
        studio.sections.clear();
        wav.tail_seconds.to_bits().hash(&mut hash);
        serde_json::to_vec(&studio)?.hash(&mut hash);
        if let Some(root) = &clip {
            for source in std::iter::once(root.as_ref()).chain(root.sources.iter().map(Arc::as_ref))
            {
                hash_audio(&mut hash, source.rate, &source.frames)?;
            }
        }
    }
    let path = directory.join(format!("{name} - {:016x}.wav", hash.finish()));
    if path.is_file() {
        return Ok(path);
    }
    files::publish_export(&path, |path| match export {
        Export::Selection { .. } => {
            let (audio, range) = selected.ok_or("The selected audio is unavailable.")?;
            files::export_cut(path, audio, range, None, wav)
        }
        Export::Pad(pad) => files::export_pad(
            path,
            clip.as_deref().ok_or("Load a sample first.")?,
            &studio,
            pad,
            wav,
        ),
        Export::Pattern => files::export_wav(path, studio, clip.clone(), wav),
    })?;
    Ok(path)
}

fn hash_audio(
    hash: &mut impl Hasher,
    rate: u32,
    frames: &[crate::audio::Frame],
) -> files::Result<()> {
    rate.hash(hash);
    frames.len().hash(hash);
    for block in frames.chunks(4096) {
        crate::background::check()?;
        for [left, right] in block {
            left.to_bits().hash(hash);
            right.to_bits().hash(hash);
        }
    }
    Ok(())
}

pub fn reveal(path: &Path) -> std::io::Result<()> {
    let mut argument = std::ffi::OsString::from("/select,");
    argument.push(path);
    std::process::Command::new("explorer.exe")
        .arg(argument)
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::{
        Com::{DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL},
        Ole::CF_HDROP,
    };

    #[test]
    fn shell_data_object_offers_a_real_file_drop() -> crate::files::Result<()> {
        let folder = tempfile::tempdir()?;
        let path = folder.path().join("prepared.wav");
        std::fs::write(&path, b"test")?;
        let _apartment = Apartment::enter()?;
        let object = data_object(&path)?;
        let format = FORMATETC {
            cfFormat: CF_HDROP.0,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        };
        unsafe { object.QueryGetData(&format).ok()? };
        Ok(())
    }

    #[test]
    fn prepared_exports_are_cached_and_cancellation_leaves_no_partial_wav() -> files::Result<()> {
        let folder = tempfile::tempdir()?;
        let settings = WavOptions::default();
        let clip = Arc::new(Clip::new(48_000, vec![[0.25; 2]; 48_000]));
        let export = Export::Selection {
            source: 0,
            range: (0.0, 0.5),
        };
        let prepare_clip = |clip| {
            prepare(
                folder.path(),
                export,
                Studio::default(),
                Some(clip),
                settings,
            )
        };
        let first = prepare_clip(clip.clone())?;
        assert_eq!(prepare_clip(clip.clone())?, first);
        let control = crate::background::Control::default();
        control.cancel();
        let other = Arc::new(Clip::new(48_000, vec![[0.1; 2]; 48_000]));
        assert!(control.run(|| prepare_clip(other.clone())).is_err());
        assert_eq!(std::fs::read_dir(folder.path())?.count(), 1);
        let second = prepare_clip(other)?;
        assert_ne!(first, second);
        assert_eq!(hound::WavReader::open(second)?.duration(), 24_000);
        Ok(())
    }
}
