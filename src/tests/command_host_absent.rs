#![cfg(target_os = "macos")]

use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

fn private_dir(label: &str) -> PathBuf {
    let path = PathBuf::from(format!("/tmp/adh-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .expect("create private dir");
    path
}

fn run(args: &[&str], tmpdir: &Path, home: &Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_agent-desktop"))
        .args(args)
        .env("TMPDIR", tmpdir)
        .env("AGENT_DESKTOP_HOME", home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("binary starts");
    let deadline = Instant::now() + Duration::from_secs(60);
    while child.try_wait().expect("poll child").is_none() {
        assert!(Instant::now() < deadline, "{args:?} did not finish in 60s");
        std::thread::sleep(Duration::from_millis(20));
    }
    child.wait_with_output().expect("collect output")
}

fn host_marked_processes(tmpdir: &Path) -> Vec<String> {
    let output = Command::new("ps")
        .args(["eww", "-A", "-o", "pid=,command="])
        .output()
        .expect("ps runs");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| {
            line.contains("AGENT_DESKTOP_INTERNAL_COMMAND_HOST")
                && line.contains(&tmpdir.display().to_string())
        })
        .map(str::to_owned)
        .collect()
}

#[test]
fn commands_never_reach_a_listening_command_host_socket() {
    let tmpdir = private_dir("armed");
    let home = private_dir("home");
    let uid = std::fs::metadata(&tmpdir).expect("stat tmpdir").uid();
    let host_dir = tmpdir.join(format!("agent-desktop-host-{uid}"));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&host_dir)
        .expect("create host dir");
    let listener = UnixListener::bind(host_dir.join("decoy.sock")).expect("bind decoy socket");
    listener.set_nonblocking(true).expect("nonblocking");
    let connections = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = {
        let (connections, stop) = (connections.clone(), stop.clone());
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if listener.accept().is_ok() {
                    connections.fetch_add(1, Ordering::SeqCst);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };

    let version = run(&["version"], &tmpdir, &home);
    assert!(version.status.success(), "{:?}", version);
    run(&["snapshot"], &tmpdir, &home);
    let batch = run(&["batch", r#"[{"command":"version"}]"#], &tmpdir, &home);
    assert!(batch.status.success(), "{:?}", batch);

    std::thread::sleep(Duration::from_millis(300));
    stop.store(true, Ordering::SeqCst);
    watcher.join().expect("watcher joins");
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "a command reached the host socket"
    );
    assert!(host_marked_processes(&tmpdir).is_empty());
    let _ = std::fs::remove_dir_all(&tmpdir);
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn commands_do_not_create_a_host_directory_or_process() {
    let tmpdir = private_dir("fresh");
    let home = private_dir("freshhome");
    let uid = std::fs::metadata(&tmpdir).expect("stat tmpdir").uid();
    run(&["version"], &tmpdir, &home);
    run(&["snapshot"], &tmpdir, &home);
    run(&["batch", r#"[{"command":"version"}]"#], &tmpdir, &home);
    assert!(!tmpdir.join(format!("agent-desktop-host-{uid}")).exists());
    assert!(host_marked_processes(&tmpdir).is_empty());
    let _ = std::fs::remove_dir_all(&tmpdir);
    let _ = std::fs::remove_dir_all(&home);
}
