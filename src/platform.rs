pub mod drag_export;

pub(crate) fn promote_audio_thread() {
    use windows::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_TIME_CRITICAL,
    };

    // CPAL 0.15.3's WASAPI worker passes a thread ID where Windows expects a handle.
    // Call on the dedicated stream thread and let Windows release the priority on exit.
    let _ = unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
}
