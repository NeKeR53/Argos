//! Job Windows : arrête tous les descendants quand le dernier handle est fermé.
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::sync::Arc;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject,
};

#[derive(Debug, Clone)]
pub struct Job(Arc<OwnedHandle>);
impl Job {
    /// `process` doit être un handle de processus vivant détenu par l'appelant.
    pub fn assign(process: RawHandle) -> io::Result<Self> {
        // SAFETY : l'objet créé est immédiatement transféré à OwnedHandle ; les pointeurs
        // correspondent aux structures Win32 et restent valides pendant les appels.
        unsafe {
            let raw = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            let handle = OwnedHandle::from_raw_handle(raw as RawHandle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
                || AssignProcessToJobObject(raw, process as _) == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self(Arc::new(handle)))
        }
    }
    /// Associe un processus créé suspendu, puis démarre son thread initial.
    pub fn assign_suspended(process: RawHandle, pid: u32) -> io::Result<Self> {
        let job = Self::assign(process)?;
        resume_initial_thread(pid)?;
        Ok(job)
    }
    pub fn kill(&self) -> io::Result<()> {
        // SAFETY : le handle appartient à l'Arc tant que cet appel est en cours.
        if unsafe { TerminateJobObject(self.0.as_raw_handle() as _, 1) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn resume_initial_thread(pid: u32) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{
        Foundation::INVALID_HANDLE_VALUE,
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First,
                Thread32Next,
            },
            Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME},
        },
    };
    // SAFETY : handles et structure Toolhelp valides pour chaque appel ; OwnedHandle les ferme.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = OwnedHandle::from_raw_handle(snapshot as RawHandle);
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
        let mut ok = Thread32First(snapshot.as_raw_handle() as _, &mut entry);
        while ok != 0 {
            if entry.th32OwnerProcessID == pid {
                let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                if thread.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let thread = OwnedHandle::from_raw_handle(thread as RawHandle);
                if ResumeThread(thread.as_raw_handle() as _) == u32::MAX {
                    return Err(io::Error::last_os_error());
                }
                return Ok(());
            }
            ok = Thread32Next(snapshot.as_raw_handle() as _, &mut entry);
        }
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "thread initial du processus introuvable",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::Job;
    use std::{
        os::windows::{io::AsRawHandle, process::CommandExt},
        process::{Command, Stdio},
        thread,
        time::Duration,
    };

    #[test]
    fn closing_job_stops_the_attached_process() {
        let mut child = Command::new("cmd.exe")
            .args(["/C", "ping -n 60 127.0.0.1 >NUL"])
            .creation_flags(0x0000_0004)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let job = Job::assign_suspended(child.as_raw_handle(), child.id()).unwrap();
        drop(job);
        for _ in 0..50 {
            if child.try_wait().unwrap().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        let _ = child.kill();
        panic!("le processus n'a pas été arrêté à la fermeture du job");
    }
}
