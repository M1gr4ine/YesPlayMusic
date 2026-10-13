//! Line-delimited JSON client for `windows-taskbar-native.exe`, which probes
//! Explorer's taskbars and parents the overlay into one of them.

use std::{
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{path::BaseDirectory, AppHandle, Manager};

use crate::taskbar_docking_model::Probe;

const REPLY_TIMEOUT: Duration = Duration::from_secs(3);
const EXIT_GRACE: Duration = Duration::from_secs(2);
const MAX_LINE_BYTES: usize = 65_536;

/// One helper process per overlay HWND, restarted on demand.
#[derive(Default)]
pub struct Helper {
    session: Option<Session>,
}

struct Session {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<String>,
    hwnd: u64,
    next_id: u64,
}

#[derive(Deserialize)]
struct Reply {
    id: Option<u64>,
    result: Option<Value>,
    error: Option<String>,
}

impl Helper {
    /// Whether a helper that may hold an attachment for `hwnd` is still running.
    pub fn is_alive_for(&mut self, hwnd: u64) -> bool {
        self.session
            .as_mut()
            .is_some_and(|session| session.hwnd == hwnd && session.is_running())
    }

    pub fn probe(&mut self, app: &AppHandle, hwnd: u64) -> Result<Probe, String> {
        let value = self.request(app, hwnd, "probe", json!({}))?;
        serde_json::from_value(value).map_err(|_| "taskbar probe is invalid".to_owned())
    }

    pub fn request(
        &mut self,
        app: &AppHandle,
        hwnd: u64,
        op: &str,
        args: Value,
    ) -> Result<Value, String> {
        if !self.is_alive_for(hwnd) {
            self.close();
            self.session = Some(Session::spawn(app, hwnd)?);
        }
        let session = self
            .session
            .as_mut()
            .expect("helper session was just started");
        session.request(op, args).map_err(|failure| {
            if failure.fatal {
                self.close();
            }
            failure.message
        })
    }

    /// Closing stdin makes the helper undo its attachment before exiting.
    pub fn close(&mut self) {
        if let Some(session) = self.session.take() {
            drop(session.stdin);
            reap(session.child);
        }
    }

    /// Like `close`, for callers on the UI thread: undoing the attachment
    /// sends messages to the overlay, which that thread must still pump.
    pub fn close_in_background(&mut self) {
        if let Some(session) = self.session.take() {
            drop(session.stdin);
            thread::spawn(move || reap(session.child));
        }
    }
}

impl Drop for Helper {
    fn drop(&mut self) {
        self.close_in_background();
    }
}

/// A stale helper must not outlive its replacement and later replay its
/// saved frame onto the overlay.
fn reap(mut child: Child) {
    let started = Instant::now();
    while matches!(child.try_wait(), Ok(None)) {
        if started.elapsed() >= EXIT_GRACE {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

struct Failure {
    message: String,
    /// The session can no longer be trusted and must be restarted.
    fatal: bool,
}

impl Failure {
    fn fatal(message: &str) -> Self {
        Self {
            message: message.to_owned(),
            fatal: true,
        }
    }
}

impl Session {
    fn spawn(app: &AppHandle, hwnd: u64) -> Result<Self, String> {
        let mut command = Command::new(helper_path(app)?);
        command
            .arg(std::process::id().to_string())
            .arg(hwnd.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command
            .spawn()
            .map_err(|error| format!("taskbar helper could not start: {error}"))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            return Err("taskbar helper pipes are unavailable".to_owned());
        };
        let (sender, replies) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            while matches!(reader.read_line(&mut line), Ok(1..=MAX_LINE_BYTES)) {
                if sender.send(std::mem::take(&mut line)).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            replies,
            hwnd,
            next_id: 0,
        })
    }

    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn request(&mut self, op: &str, mut args: Value) -> Result<Value, Failure> {
        self.next_id += 1;
        let id = self.next_id;
        args["id"] = id.into();
        args["op"] = op.into();
        let mut line = args.to_string();
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|()| self.stdin.flush())
            .map_err(|_| Failure::fatal("taskbar helper stopped"))?;

        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let line = self
                .replies
                .recv_timeout(remaining)
                .map_err(|_| Failure::fatal("taskbar helper did not reply"))?;
            let reply: Reply = serde_json::from_str(&line)
                .map_err(|_| Failure::fatal("taskbar helper reply is invalid"))?;
            if reply.id != Some(id) {
                continue;
            }
            return match (reply.result, reply.error) {
                (Some(result), None) => Ok(result),
                (_, error) => Err(Failure {
                    message: error.unwrap_or_else(|| "taskbar helper failed".to_owned()),
                    fatal: false,
                }),
            };
        }
    }
}

fn helper_path(app: &AppHandle) -> Result<PathBuf, String> {
    let bundled = app
        .path()
        .resolve("windows-taskbar-native.exe", BaseDirectory::Resource)
        .ok()
        .filter(|path| path.is_file());
    #[cfg(debug_assertions)]
    let bundled = bundled.or_else(|| {
        Some(PathBuf::from(env!("TASKBAR_NATIVE_HELPER_PATH"))).filter(|path| path.is_file())
    });
    bundled.ok_or_else(|| "taskbar helper is missing".to_owned())
}
