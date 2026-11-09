use crate::files::Result;
use std::{
    io,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    process::{Child, Command},
};
use windows::{
    Win32::{
        Foundation::HANDLE,
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            Threading::{
                CREATE_NO_WINDOW, CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
            },
        },
    },
    core::PCWSTR,
};

pub(super) fn configure(command: &mut Command) {
    // A launcher can start helpers before spawn returns. Bind the job first.
    command.creation_flags(CREATE_NO_WINDOW.0 | CREATE_SUSPENDED.0);
}

pub(super) fn bind(child: &Child) -> Result<OwnedHandle> {
    // Each successful handle acquisition is immediately wrapped for cleanup on error.
    unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null())?;
        let owner = OwnedHandle::from_raw_handle(job.0);
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            std::ptr::from_ref(&limits).cast(),
            std::mem::size_of_val(&limits) as u32,
        )?;
        AssignProcessToJobObject(job, HANDLE(child.as_raw_handle()))?;
        Ok(owner)
    }
}

pub(super) fn resume(child: &Child) -> Result<()> {
    // std::process::Child owns the process handle. Find its suspended primary thread.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0)?;
        let _snapshot = OwnedHandle::from_raw_handle(snapshot.0);
        let size = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut entry = THREADENTRY32 {
            dwSize: size,
            ..Default::default()
        };
        Thread32First(snapshot, &mut entry)?;
        while entry.th32OwnerProcessID != child.id() {
            entry.dwSize = size;
            Thread32Next(snapshot, &mut entry)?;
        }
        let thread = OpenThread(THREAD_SUSPEND_RESUME, false, entry.th32ThreadID)?;
        let _thread = OwnedHandle::from_raw_handle(thread.0);
        match ResumeThread(thread) {
            1 => Ok(()),
            u32::MAX => Err(io::Error::last_os_error().into()),
            _ => Err("Unexpected worker thread suspension count.".into()),
        }
    }
}
