//! Current-user control ownership. The lease covers settings and routing together.
use crate::policy::WindowsAudioPolicy;
use jack_core::{
    routing::{FileJournal, Role, RoutingError},
    runtime::{AudioEngine, RuntimeController},
};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::windows::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};
use windows::Win32::UI::Shell::{FOLDERID_LocalAppData, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
pub struct ControlLease {
    _file: File,
}
impl ControlLease {
    pub fn acquire(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        Ok(Self {
            _file: OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .share_mode(0)
                .open(path)?,
        })
    }
}
pub fn lease_is_busy(error: &io::Error) -> bool {
    matches!(error.raw_os_error(), Some(32 | 33))
}
pub fn current_user_root() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let local = unsafe { SHGetKnownFolderPath(&FOLDERID_LocalAppData, KF_FLAG_DEFAULT, None)? };
    Ok(PathBuf::from(crate::platform::owned_string(local)?).join("Jack"))
}
pub struct RuntimeOwner<E: AudioEngine> {
    controller: RuntimeController<WindowsAudioPolicy, FileJournal, E>,
    settings_path: PathBuf,
    _lease: ControlLease,
}
impl<E: AudioEngine> RuntimeOwner<E> {
    /// Use on the COM control thread. Global to Jack's current-user data directory,
    /// independent of a chosen settings/journal path or endpoint. No policy writes.
    pub fn new(engine: E, roles: Vec<Role>) -> Result<Self, Box<dyn std::error::Error>> {
        let root = current_user_root()?;
        let lease = ControlLease::acquire(&root.join("control.lock"))?;
        let policy = WindowsAudioPolicy::new()?;
        let store = FileJournal {
            path: root.join("routes.json"),
        };
        let controller = RuntimeController::new(policy, store, engine, roles)?;
        Ok(Self {
            controller,
            settings_path: root.join("settings.json"),
            _lease: lease,
        })
    }
    pub fn settings_path(&self) -> &Path {
        &self.settings_path
    }
    pub fn controller(&mut self) -> &mut RuntimeController<WindowsAudioPolicy, FileJournal, E> {
        &mut self.controller
    }
    /// Runtime PID only: adapter retains a handle and verifies live identity.
    pub fn bind_process(&mut self, pid: u32) -> Result<String, RoutingError> {
        self.controller
            .with_policy_adapter(|policy| policy.bind_process(pid))
    }
    /// Caller must check complete before exiting; Drop cannot guarantee restoration.
    pub fn shutdown(
        &mut self,
    ) -> Result<jack_core::routing::RecoveryReport, jack_core::runtime::RuntimeError> {
        self.controller.shutdown()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Write},
        os::windows::process::CommandExt,
        process::{Child, Command, Stdio},
        sync::mpsc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "jack-lease-{}-{}.lock",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .unwrap();
            Self(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    struct HeldChild(Child);
    impl Drop for HeldChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    fn child(path: &Path, hold: bool) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--ignored",
                "--exact",
                "control::tests::lease_child",
                "--nocapture",
            ])
            .env("JACK_TEST_LEASE_PATH", path)
            .env("JACK_TEST_LEASE_HOLD", if hold { "1" } else { "0" })
            .creation_flags(0x08000000);
        command
    }
    #[test]
    #[ignore = "helper invoked only by the two parent process tests"]
    fn lease_child() {
        let Some(path) = std::env::var_os("JACK_TEST_LEASE_PATH") else {
            return;
        };
        let lease = match ControlLease::acquire(Path::new(&path)) {
            Ok(lease) => lease,
            Err(error) if lease_is_busy(&error) => std::process::exit(22),
            Err(error) => panic!("lease child: {error}"),
        };
        if std::env::var("JACK_TEST_LEASE_HOLD").as_deref() == Ok("1") {
            println!("JACK_LEASE_HELD");
            std::io::stdout().flush().unwrap();
            std::thread::sleep(Duration::from_secs(30));
        }
        drop(lease);
    }
    #[test]
    fn exclusive_lease_blocks_another_process_and_releases_normally() {
        let path = Scratch::new();
        let lease = ControlLease::acquire(&path.0).unwrap();
        let blocked = child(&path.0, false).output().unwrap();
        assert_eq!(blocked.status.code(), Some(22));
        drop(lease);
        assert!(child(&path.0, false).output().unwrap().status.success());
    }
    #[test]
    fn killed_owner_releases_lease_without_deleting_persistent_lock_file() {
        let path = Scratch::new();
        let mut child = HeldChild(
            child(&path.0, true)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let stdout = child.0.stdout.take().unwrap();
        let (sender, receiver) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if line.unwrap().contains("JACK_LEASE_HELD") {
                    let _ = sender.send(());
                    break;
                }
            }
        });
        assert!(receiver.recv_timeout(Duration::from_secs(3)).is_ok());
        reader.join().unwrap();
        let error = ControlLease::acquire(&path.0)
            .err()
            .expect("child holds lease");
        assert!(lease_is_busy(&error));
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        assert!(path.0.exists());
        let after = ControlLease::acquire(&path.0).unwrap();
        drop(after);
    }
}
