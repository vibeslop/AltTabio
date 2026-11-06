//! Native fixtures use unique names, never the installed application's singleton or UI.
use super::*;
use crate::win32::wide;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_HANDLE, HLOCAL, LocalFree, WAIT_OBJECT_0,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{SendMessageW, WM_CLOSE};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Names {
    mutex: Vec<u16>,
    class: Vec<u16>,
}

impl Names {
    fn new() -> Self {
        let suffix = format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        Self {
            mutex: wide(&format!("Local\\AltTabio.SingletonTest-{suffix}")),
            class: wide(&format!("AltTabio.SingletonTest-{suffix}")),
        }
    }

    fn acquire(&self) -> Result<Option<SingleInstance>> {
        SingleInstance::acquire_named(
            PCWSTR(self.mutex.as_ptr()),
            PCWSTR(self.class.as_ptr()),
            Duration::ZERO,
        )
    }
}

fn collision(result: Result<Option<SingleInstance>>) {
    match result {
        Err(error) => assert!(
            error
                .message()
                .contains("could not verify another running instance")
        ),
        Ok(_) => panic!("Unverified object was accepted as a genuine instance or taken over"),
    }
}

#[test]
fn unowned_precreated_mutex_is_a_visible_conflict_and_is_not_taken_over() -> Result<()> {
    let names = Names::new();
    let handle = unsafe {
        // SAFETY: names remains live; this fixture owns the new, initially unowned handle.
        CreateMutexW(None, false, PCWSTR(names.mutex.as_ptr()))
    }?;
    let handle = OwnedProcess(handle);
    collision(names.acquire());
    // A failed acquisition must not close somebody else's handle or change the object's type.
    let waited = unsafe {
        // SAFETY: the fixture's handle is still owned and the wait cannot block.
        WaitForSingleObject(handle.0, 0)
    };
    assert_eq!(waited, WAIT_OBJECT_0);
    let owned = OwnedMutex {
        handle: handle.0,
        _same_thread: PhantomData,
    };
    std::mem::forget(handle); // Transfer the fixture's one owned handle, now with mutex ownership.
    drop(owned);
    assert!(names.acquire()?.is_some());
    Ok(())
}

#[test]
fn wrong_object_type_is_a_visible_conflict() -> Result<()> {
    let names = Names::new();
    let event = unsafe {
        // SAFETY: the unique terminated name is live; the event handle is fixture-owned.
        CreateEventW(None, true, false, PCWSTR(names.mutex.as_ptr()))
    }?;
    let _event = OwnedProcess(event);
    let reopened = unsafe {
        // SAFETY: the test name is live; an event occupies this name, so no mutex is opened.
        CreateMutexW(None, true, PCWSTR(names.mutex.as_ptr()))
    };
    assert_eq!(
        reopened.err().map(|error| error.code()),
        Some(ERROR_INVALID_HANDLE.to_hresult())
    );
    collision(names.acquire());
    Ok(())
}

#[test]
fn genuine_duplicate_and_relaunch_preserve_guard_ownership() -> Result<()> {
    let names = Names::new();
    let first = names
        .acquire()?
        .ok_or_else(|| conflict("Missing first guard"))?;
    assert!(names.acquire()?.is_none());
    assert!(names.acquire()?.is_none());
    drop(first);
    assert!(names.acquire()?.is_some());
    Ok(())
}

#[test]
fn foreign_close_messages_cannot_destroy_the_owned_rendezvous() -> Result<()> {
    let names = Names::new();
    let peer = PeerWindow::create(PCWSTR(names.class.as_ptr()))?;
    unsafe {
        // SAFETY: this test sends a pointer-free message only to its own fixture window.
        SendMessageW(peer.window, WM_CLOSE, None, None);
    }
    assert!(authenticated_peer(PCWSTR(names.class.as_ptr()))?);
    drop(peer);
    assert!(!authenticated_peer(PCWSTR(names.class.as_ptr()))?);
    Ok(())
}

#[test]
fn failed_peer_creation_releases_the_new_mutex() -> Result<()> {
    let names = Names::new();
    let peer = PeerWindow::create(PCWSTR(names.class.as_ptr()))?;
    assert!(names.acquire().is_err()); // Class registration fails after creating the mutex.
    drop(peer);
    assert!(names.acquire()?.is_some());
    Ok(())
}

#[test]
fn genuine_instance_in_another_process_is_authenticated() -> Result<()> {
    let names = Names::new();
    let _fixture = Fixture::spawn(&names, "genuine", false)?;
    assert!(names.acquire()?.is_none());
    Ok(())
}

#[test]
fn owned_precreated_mutex_in_another_process_is_a_visible_conflict() -> Result<()> {
    let names = Names::new();
    let _fixture = Fixture::spawn(&names, "mutex", false)?;
    collision(names.acquire());
    Ok(())
}

#[test]
fn matching_rendezvous_class_from_a_different_executable_is_rejected() -> Result<()> {
    let names = Names::new();
    // The copy even has identical bytes and a matching window class. Its file identity differs.
    let _fixture = Fixture::spawn(&names, "genuine", true)?;
    collision(names.acquire());
    Ok(())
}

#[test]
fn access_denied_is_a_conflict_unless_a_genuine_peer_is_verified() -> Result<()> {
    let names = Names::new();
    let _fixture = Fixture::spawn(&names, "denied", false)?;
    let reopened = unsafe {
        // SAFETY: the test name is live. The fixture's protected empty DACL denies this open.
        CreateMutexW(None, true, PCWSTR(names.mutex.as_ptr()))
    };
    assert_eq!(
        reopened.err().map(|error| error.code()),
        Some(ERROR_ACCESS_DENIED.to_hresult())
    );
    collision(names.acquire());
    let peer = PeerWindow::create(PCWSTR(names.class.as_ptr()))?;
    assert!(names.acquire()?.is_none());
    drop(peer);
    Ok(())
}

#[test]
fn concurrent_startup_waits_for_the_winning_launch_to_publish_its_peer() -> Result<()> {
    for mode in ["delayed", "delayed-denied"] {
        let names = Names::new();
        let _fixture = Fixture::spawn(&names, mode, false)?;
        assert!(
            SingleInstance::acquire_named(
                PCWSTR(names.mutex.as_ptr()),
                PCWSTR(names.class.as_ptr()),
                STARTUP_GRACE,
            )?
            .is_none()
        );
    }
    Ok(())
}

struct Fixture {
    child: Child,
    folder: PathBuf,
}

impl Fixture {
    fn spawn(names: &Names, mode: &str, copy: bool) -> Result<Self> {
        let suffix = NEXT.fetch_add(1, Ordering::Relaxed);
        let folder = std::env::temp_dir().join(format!(
            "AltTabio-singleton-test-{}-{suffix}",
            std::process::id(),
        ));
        std::fs::create_dir(&folder)?;
        let executable = std::env::current_exe()?;
        let executable = if copy {
            let target = folder.join("AltTabio.exe");
            std::fs::copy(executable, &target)?;
            target
        } else {
            executable
        };
        let child = Command::new(executable)
            .args([
                "--ignored",
                "--exact",
                "single_instance::tests::peer_fixture",
                "--quiet",
            ])
            .env("ALTTABIO_SINGLETON_FIXTURE", &folder)
            .env("ALTTABIO_SINGLETON_MODE", mode)
            .env(
                "ALTTABIO_SINGLETON_MUTEX",
                String::from_utf16_lossy(&names.mutex[..names.mutex.len() - 1]),
            )
            .env(
                "ALTTABIO_SINGLETON_CLASS",
                String::from_utf16_lossy(&names.class[..names.class.len() - 1]),
            )
            .stdout(Stdio::null())
            .spawn()?;
        let mut fixture = Self { child, folder };
        let started = Instant::now();
        while !fixture.folder.join("ready").exists() {
            assert!(
                fixture.child.try_wait()?.is_none(),
                "Singleton fixture exited before readiness"
            );
            assert!(
                started.elapsed() < Duration::from_secs(15),
                "Singleton fixture timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(fixture)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = self.child.kill() {
            eprintln!("Could not stop singleton fixture: {error}");
        }
        if let Err(error) = self.child.wait() {
            eprintln!("Could not reap singleton fixture: {error}");
        }
        if let Err(error) = std::fs::remove_dir_all(&self.folder) {
            eprintln!("Could not remove singleton fixture: {error}");
        }
    }
}

#[test]
#[ignore = "Subprocess fixture invoked only by the singleton regression tests"]
fn peer_fixture() -> Result<()> {
    let folder = PathBuf::from(std::env::var_os("ALTTABIO_SINGLETON_FIXTURE").ok_or(E_FAIL)?);
    let mode = std::env::var("ALTTABIO_SINGLETON_MODE").map_err(|_| Error::from(E_FAIL))?;
    let mutex = wide(&std::env::var("ALTTABIO_SINGLETON_MUTEX").map_err(|_| Error::from(E_FAIL))?);
    let class = wide(&std::env::var("ALTTABIO_SINGLETON_CLASS").map_err(|_| Error::from(E_FAIL))?);
    let guard;
    let _mutex;
    let _peer;
    if mode == "genuine" {
        guard = SingleInstance::acquire_named(
            PCWSTR(mutex.as_ptr()),
            PCWSTR(class.as_ptr()),
            Duration::ZERO,
        )?;
        assert!(guard.is_some());
    } else {
        _mutex = fixture_mutex(
            PCWSTR(mutex.as_ptr()),
            mode == "denied" || mode == "delayed-denied",
        )?;
        if mode == "delayed" || mode == "delayed-denied" {
            std::fs::write(folder.join("ready"), [])?;
            std::thread::sleep(Duration::from_millis(150));
            _peer = PeerWindow::create(PCWSTR(class.as_ptr()))?;
        }
    }
    std::fs::write(folder.join("ready"), [])?;
    // No message loop is needed: peer discovery never sends a message to this thread.
    std::thread::sleep(Duration::from_secs(30));
    Ok(())
}

fn fixture_mutex(name: PCWSTR, denied: bool) -> Result<OwnedMutex> {
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    if denied {
        unsafe {
            // SAFETY: static SDDL and a writable output. This empty protected DACL denies opens,
            // while the creator retains its initial handle for the fixture's full lifetime.
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                w!("D:P"),
                SDDL_REVISION_1,
                &raw mut descriptor,
                None,
            )?;
        }
    }
    let attributes = SECURITY_ATTRIBUTES {
        nLength: u32::try_from(std::mem::size_of::<SECURITY_ATTRIBUTES>())
            .map_err(|_| Error::from(E_FAIL))?,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let created = unsafe {
        // SAFETY: name and the optional descriptor are live. The returned fresh mutex is owned.
        CreateMutexW(
            if denied {
                Some(&raw const attributes)
            } else {
                None
            },
            true,
            name,
        )
    };
    if denied {
        let freed = unsafe {
            // SAFETY: the converter allocated this descriptor with LocalAlloc; release it once.
            LocalFree(Some(HLOCAL(descriptor.0)))
        };
        assert!(freed.0.is_null());
    }
    Ok(OwnedMutex {
        handle: created?,
        _same_thread: PhantomData,
    })
}

#[test]
fn peer_disappearance_during_authentication_retries_the_startup_gate() -> Result<()> {
    let names = Names::new();
    let mut departing = Some(fixture_mutex(PCWSTR(names.mutex.as_ptr()), false)?);
    let mut probes = 0;
    let acquired = SingleInstance::acquire_with_peer_probe(
        PCWSTR(names.mutex.as_ptr()),
        PCWSTR(names.class.as_ptr()),
        Duration::from_millis(250),
        |_| {
            probes += 1;
            drop(departing.take());
            Err(conflict("The peer exited during process authentication"))
        },
    )?;
    assert!(
        acquired.is_some(),
        "The released gate must allow a fresh launch"
    );
    assert_eq!(probes, 1);
    Ok(())
}

#[test]
fn persistent_peer_authentication_errors_remain_visible_at_the_deadline() -> Result<()> {
    let names = Names::new();
    let _occupied = fixture_mutex(PCWSTR(names.mutex.as_ptr()), false)?;
    let result = SingleInstance::acquire_with_peer_probe(
        PCWSTR(names.mutex.as_ptr()),
        PCWSTR(names.class.as_ptr()),
        Duration::ZERO,
        |_| Err(conflict("Peer query deliberately denied")),
    );
    match result {
        Err(error) => assert!(error.message().contains("Peer query deliberately denied")),
        Ok(_) => panic!("An unverified peer must not authorize a quiet duplicate exit"),
    }
    Ok(())
}
