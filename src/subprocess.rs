use crate::{background, files::Result};
use std::{
    io::{self, Read},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

#[cfg(windows)]
mod windows_job;

const POLL_INTERVAL: Duration = Duration::from_millis(25);

pub struct Process {
    pub child: Child,
    #[cfg(unix)]
    owns_group: bool,
    #[cfg(windows)]
    job: Option<std::os::windows::io::OwnedHandle>,
}

impl Process {
    pub fn spawn(command: &mut Command) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        #[cfg(windows)]
        windows_job::configure(command);
        let process = Self {
            child: command.spawn()?,
            #[cfg(unix)]
            owns_group: true,
            #[cfg(windows)]
            job: None,
        };
        #[cfg(windows)]
        let process = {
            let mut process = process;
            process.job = Some(windows_job::bind(&process.child)?);
            windows_job::resume(&process.child)?;
            process
        };
        Ok(process)
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if self.owns_group {
            // Observe exit without reaping the group leader. Its ID stays reserved
            // until the remaining group has been signalled.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let result = unsafe {
                libc::waitid(
                    libc::P_PID,
                    self.child.id() as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                )
            };
            if result == -1 {
                let error = io::Error::last_os_error();
                return if error.kind() == io::ErrorKind::Interrupted {
                    Ok(None)
                } else {
                    Err(error)
                };
            }
            if unsafe { info.si_pid() } == 0 {
                return Ok(None);
            }
            self.kill_group();
        }
        let status = self.child.try_wait()?;
        #[cfg(unix)]
        if status.is_some() {
            self.owns_group = false;
        }
        Ok(status)
    }

    #[cfg(unix)]
    fn kill_group(&mut self) {
        if std::mem::take(&mut self.owns_group) {
            // The child was started as the leader of its own process group.
            unsafe { libc::killpg(self.child.id() as libc::pid_t, libc::SIGKILL) };
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        #[cfg(windows)]
        drop(self.job.take());
        #[cfg(unix)]
        self.kill_group();
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[derive(Default)]
pub struct Capture {
    pub stdout: usize,
    pub stderr: usize,
}

pub struct Output {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: String,
}

pub fn run(
    command: &mut Command,
    name: &str,
    timeout: Duration,
    capture: Capture,
) -> Result<Output> {
    let deadline = Instant::now() + timeout;
    command
        .stdin(Stdio::null())
        .stdout(output_pipe(capture.stdout))
        .stderr(output_pipe(capture.stderr));
    background::check()?;
    let mut process =
        Process::spawn(command).map_err(|error| format!("Could not start {name}. {error}"))?;
    let stdout = process
        .child
        .stdout
        .take()
        .map(|pipe| drain(pipe, capture.stdout, false));
    let stderr = process
        .child
        .stderr
        .take()
        .map(|pipe| drain(pipe, capture.stderr, true));
    let status = loop {
        let remaining = time_left(name, deadline)?;
        if let Some(status) = process.try_wait()? {
            break status;
        }
        thread::sleep(remaining.min(POLL_INTERVAL));
    };
    // Close inherited pipes from any descendants before waiting for the readers.
    drop(process);
    let stdout = collect(stdout, name, deadline)?;
    let stderr = collect(stderr, name, deadline)?;
    if status.success() && stdout.truncated {
        return Err(format!("{name} returned too much output.").into());
    }
    Ok(Output {
        status,
        stdout: stdout.bytes,
        stderr: String::from_utf8_lossy(&stderr.bytes).into_owned(),
    })
}

fn output_pipe(limit: usize) -> Stdio {
    if limit == 0 {
        Stdio::null()
    } else {
        Stdio::piped()
    }
}

fn time_left(name: &str, deadline: Instant) -> Result<Duration> {
    background::check()?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(io::Error::new(io::ErrorKind::TimedOut, format!("{name} timed out.")).into());
    }
    Ok(remaining)
}

#[derive(Default)]
struct Captured {
    bytes: Vec<u8>,
    truncated: bool,
}

fn drain(
    pipe: impl Read + Send + 'static,
    limit: usize,
    tail: bool,
) -> Receiver<io::Result<Captured>> {
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let _ = send.send(read_bounded(pipe, limit, tail));
    });
    receive
}

fn read_bounded(mut pipe: impl Read, limit: usize, tail: bool) -> io::Result<Captured> {
    let mut capture = Captured::default();
    let mut buffer = [0; 8192];
    loop {
        let count = match pipe.read(&mut buffer) {
            Ok(0) => return Ok(capture),
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        capture.truncated |= count > limit - capture.bytes.len();
        if tail {
            let start = count.saturating_sub(limit);
            let discard = (capture.bytes.len() + count - start).saturating_sub(limit);
            capture.bytes.drain(..discard);
            capture.bytes.extend_from_slice(&buffer[start..count]);
        } else {
            let keep = count.min(limit - capture.bytes.len());
            capture.bytes.extend_from_slice(&buffer[..keep]);
        }
    }
}

fn collect(
    reader: Option<Receiver<io::Result<Captured>>>,
    name: &str,
    deadline: Instant,
) -> Result<Captured> {
    let Some(reader) = reader else {
        return Ok(Captured::default());
    };
    loop {
        let remaining = time_left(name, deadline)?;
        match reader.recv_timeout(remaining.min(POLL_INTERVAL)) {
            Ok(result) => return Ok(result?),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err("Process output reader stopped unexpectedly.".into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const FIXTURE: &str = "LOST_SAMPLES_SUBPROCESS_FIXTURE";

    fn fixture(mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "subprocess::tests::subprocess_fixture",
                "--nocapture",
            ])
            .env(FIXTURE, mode);
        command
    }

    #[test]
    fn subprocess_fixture() {
        match std::env::var(FIXTURE).ok().as_deref() {
            Some("failure") => {
                println!("result marker");
                let mut stderr = io::stderr().lock();
                stderr.write_all(&[b'x'; 128 * 1024]).unwrap();
                stderr.write_all(b"\nlast error\n").unwrap();
                std::process::exit(7);
            }
            Some("overflow") => {
                io::stdout().write_all(&[b'x'; 128 * 1024]).unwrap();
                std::process::exit(0);
            }
            Some("wait") => thread::sleep(Duration::from_secs(10)),
            #[cfg(windows)]
            Some("tree") => {
                let mut child = fixture("wait").spawn().unwrap();
                let path = std::path::PathBuf::from(
                    std::env::var_os("LOST_SAMPLES_SUBPROCESS_PID").unwrap(),
                );
                std::fs::write(path, child.id().to_string()).unwrap();
                child.wait().unwrap();
            }
            _ => {}
        }
    }

    #[test]
    fn captures_only_the_stderr_tail_and_preserves_a_failed_exit() -> Result<()> {
        let output = run(
            &mut fixture("failure"),
            "test subprocess",
            Duration::from_secs(5),
            Capture {
                stdout: 2048,
                stderr: 64,
            },
        )?;
        assert_eq!(output.status.code(), Some(7));
        assert!(String::from_utf8_lossy(&output.stdout).contains("result marker"));
        assert_eq!(output.stderr.len(), 64);
        assert!(output.stderr.ends_with("last error\n"));
        Ok(())
    }

    #[test]
    fn oversized_success_output_is_reported_and_a_hung_process_times_out() {
        let overflow = run(
            &mut fixture("overflow"),
            "test subprocess",
            Duration::from_secs(5),
            Capture {
                stdout: 128,
                stderr: 0,
            },
        );
        assert_eq!(
            overflow.err().unwrap().to_string(),
            "test subprocess returned too much output."
        );

        let timeout = run(
            &mut fixture("wait"),
            "test subprocess",
            Duration::from_millis(200),
            Capture::default(),
        )
        .err()
        .unwrap();
        assert_eq!(
            timeout.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::TimedOut
        );
    }

    #[cfg(windows)]
    #[test]
    fn cancelling_the_runner_terminates_an_already_running_grandchild() -> Result<()> {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows::Win32::{
            Foundation::{HANDLE, WAIT_OBJECT_0},
            System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject},
        };

        let dir = tempfile::tempdir()?;
        let path = dir.path().join("child.pid");
        let control = background::Control::default();
        let worker_control = control.clone();
        let watch_path = path.clone();
        let watcher = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let process = loop {
                if let Ok(contents) = std::fs::read_to_string(&watch_path)
                    && let Ok(pid) = contents.parse::<u32>()
                {
                    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) }.unwrap();
                    break unsafe { OwnedHandle::from_raw_handle(handle.0) };
                }
                assert!(
                    Instant::now() < deadline,
                    "fixture did not start its grandchild"
                );
                thread::sleep(POLL_INTERVAL);
            };
            worker_control.cancel();
            process
        });
        let result = control.run(|| {
            run(
                fixture("tree").env("LOST_SAMPLES_SUBPROCESS_PID", &path),
                "test subprocess",
                Duration::from_secs(6),
                Capture::default(),
            )
        });
        let process = watcher.join().unwrap();
        let error = result.err().unwrap();
        assert_eq!(
            error.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::Interrupted
        );
        assert_eq!(
            unsafe { WaitForSingleObject(HANDLE(process.as_raw_handle()), 2000) },
            WAIT_OBJECT_0,
        );
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_batch_launchers_keep_their_working_directory_and_output() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("worker launcher.cmd");
        std::fs::write(&path, b"@echo off\r\ntype marker.txt\r\n")?;
        std::fs::write(dir.path().join("marker.txt"), b"local worker marker")?;
        let output = run(
            Command::new(path).current_dir(dir.path()),
            "test batch launcher",
            Duration::from_secs(5),
            Capture {
                stdout: 1024,
                stderr: 1024,
            },
        )?;
        assert!(output.status.success(), "{}", output.stderr);
        assert_eq!(output.stdout, b"local worker marker");
        Ok(())
    }
}
