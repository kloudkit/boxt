use std::io;

#[cfg(windows)]
pub fn suppress_error_dialogs() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
    };
    unsafe {
        SetErrorMode(SEM_NOGPFAULTERRORBOX | SEM_FAILCRITICALERRORS);
    }
}

#[cfg(not(windows))]
pub fn suppress_error_dialogs() {}

#[cfg(unix)]
pub fn set_memory_cap(bytes: u64) -> io::Result<()> {
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    let hard = getrlimit(Resource::As).maximum;
    let cap = hard.map_or(bytes, |hard| hard.min(bytes));
    setrlimit(
        Resource::As,
        Rlimit {
            current: Some(cap),
            maximum: Some(cap),
        },
    )?;
    Ok(())
}

#[cfg(windows)]
pub fn set_memory_cap(bytes: u64) -> io::Result<()> {
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION,
        JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let limit = usize::try_from(bytes).unwrap_or(usize::MAX);
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(io::Error::last_os_error());
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_PROCESS_MEMORY | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION;
        info.ProcessMemoryLimit = limit;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        if AssignProcessToJobObject(job, GetCurrentProcess()) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn die_with_parent(parent: Option<u32>) -> io::Result<()> {
    use rustix::process::{Pid, Signal, getppid, set_parent_process_death_signal};
    set_parent_process_death_signal(Some(Signal::KILL))?;
    if let Some(parent) = parent
        && Pid::as_raw(getppid()) as u32 != parent
    {
        return Err(io::Error::other(
            "the parent exited before the worker started",
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn die_with_parent(_parent: Option<u32>) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
pub fn kill_workers_on_exit() -> io::Result<()> {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    // The job handle is never closed: closing it would kill this process with its workers.
    static JOINED: OnceLock<Result<(), i32>> = OnceLock::new();
    let joined = JOINED.get_or_init(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return Err(io::Error::last_os_error().raw_os_error().unwrap_or(0));
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) == 0
            || AssignProcessToJobObject(job, GetCurrentProcess()) == 0
        {
            return Err(io::Error::last_os_error().raw_os_error().unwrap_or(0));
        }
        Ok(())
    });
    joined.map_err(io::Error::from_raw_os_error)
}

#[cfg(not(windows))]
pub fn kill_workers_on_exit() -> io::Result<()> {
    Ok(())
}
