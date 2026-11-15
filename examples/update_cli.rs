//! update_cli: drives the app's `UpdateStore` (the same core code) against a release feed, playing the app's steps
//! (dev/test only; used by `scripts/update-roundtrip.sh` with `scripts/mock-releases.py`). Plain std: runs on Linux.
//!
//!   update_cli <command> --root DIR --target EXE --feed URL
//!   launch [--crash]   what the app does first at launch: tidy, file the old exe, roll back a failing version;
//!                      then report healthy, unless --crash (the launch "dies" before it)
//!   check              what Check Now / a scheduled check does: read the feed and say what it found (no download)
//!   install            what "Update and Restart" does after the dialog: download, verify, swap
//!   install-start-fails  the same, but the new exe cannot be started: the swap is undone
//!   rollback           "Go Back to …"
//!   status             installed version, staged and previous copies, skipped versions, trial
//! Fake exes say their version in their bytes ("MZ fake NachoCombo 0.5.1"); the app reads the version resource.
// The core is linted in the app build; here it is only compiled again for the round trip.
#![allow(dead_code, clippy::all)]
#[path = "../src/core/mod.rs"]
mod core;

use crate::core::update::{self, Decision, Release, Version};
use crate::core::update_store::{ExeInfo, Fetch, LaunchAction, UpdateStore};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

struct FakeExe;
impl ExeInfo for FakeExe {
    fn version_of(&self, exe: &Path) -> Option<String> {
        let b = std::fs::read(exe).ok()?;
        String::from_utf8_lossy(&b).strip_prefix("MZ fake NachoCombo ").map(|v| v.trim().to_string())
    }
}

/// HTTP/1.0 GET for the mock server on this PC (the app uses WinHTTP).
struct LoopbackHttp;
impl Fetch for LoopbackHttp {
    fn fetch(&self, url: &str) -> Option<Vec<u8>> {
        let u = update::parse_url(url).filter(|u| !u.secure && (u.host == "127.0.0.1" || u.host == "localhost"))?;
        let mut s = std::net::TcpStream::connect(("127.0.0.1", u.port)).ok()?;
        write!(s, "GET {} HTTP/1.0\r\nHost: {}\r\nUser-Agent: NachoCombo/test\r\n\r\n", u.path, u.host).ok()?;
        let mut raw = vec![];
        s.read_to_end(&mut raw).ok()?;
        let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
        let head = String::from_utf8_lossy(&raw[..split]);
        head.lines().next().filter(|l| l.split_whitespace().nth(1) == Some("200"))?;
        Some(raw[split + 4..].to_vec())
    }
}

fn fail(msg: &str) -> ! { eprintln!("{msg}"); std::process::exit(2) }

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().cloned().unwrap_or_else(|| fail("usage: update_cli launch|check|install|install-start-fails|rollback|status --root DIR --target EXE --feed URL"));
    let opt = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let (Some(root), Some(target)) = (opt("--root"), opt("--target")) else { fail("--root and --target are required") };
    let feed = opt("--feed").unwrap_or_default();
    let crash = args.iter().any(|a| a == "--crash");
    let store = UpdateStore::new(&root);
    let target = PathBuf::from(target);
    let Some(current) = FakeExe.version_of(&target).and_then(|v| Version::parse(&v)) else { fail(&format!("no exe at {}", target.display())) };

    match command.as_str() {
        "status" => {}
        "launch" => match store.launch_began(&current, &target, &FakeExe) {
            LaunchAction::Normal => {
                println!("launch {current}: normal");
                if let Some(n) = store.take_notice(&current) { println!("notice {n:?}"); }
                if crash { println!("launch died before healthy"); status(&store, &target); std::process::exit(3); }
                store.launch_healthy(&current);
                println!("healthy {current}");
            }
            LaunchAction::RollBack { from, to } => {
                println!("launch {current}: {from} did not start properly twice, rolling back to {to}");
                match store.roll_back(&target, &current, &FakeExe) {
                    Ok(v) => println!("rolled back to {v} (restart)"),
                    Err(p) => println!("rollback failed: {p}"),
                }
            }
        },
        "check" | "install" | "install-start-fails" => {
            if !update::feed_allowed(&feed) { fail("feed refused") }
            let Some(release) = LoopbackHttp.fetch(&feed).and_then(|d| Release::parse(&d)) else { println!("check: feed unreachable"); status(&store, &target); std::process::exit(1) };
            match update::decide(&current.to_string(), &release, &store.ledger().skipped) {
                Decision::UpToDate => println!("check: up to date ({current})"),
                Decision::Ignored(why) => println!("check: ignored ({why})"),
                Decision::Available { version, zip, sums } if command == "check" => {
                    println!("check: {version} available ({}, {} bytes); nothing downloaded until the user confirms", zip.name, zip.size);
                    let _ = sums;
                }
                Decision::Available { version, zip, sums } => {
                    println!("install: user confirmed {version}, downloading");
                    match store.stage(&version, &zip, &sums, &feed, &LoopbackHttp, &FakeExe) {
                        Err(p) => println!("install refused: {p} Nothing was changed."),
                        Ok(copy) => {
                            println!("verified {} (size, sha256, exe version)", copy.version);
                            let before = store.ledger();
                            match store.apply_staged(&target, &current, &FakeExe) {
                                Err(p) => println!("install failed: {p}"),
                                Ok(v) if command == "install-start-fails" => {
                                    println!("swapped {v}, but the new exe could not be started");
                                    match store.undo_swap(&target, &before, &v, false) { Ok(()) => println!("undone: {current} kept"), Err(p) => println!("undo failed: {p}") }
                                }
                                Ok(v) => println!("applied {v} (restart)"),
                            }
                        }
                    }
                }
            }
        }
        "rollback" => match store.roll_back(&target, &current, &FakeExe) {
            Ok(v) => println!("rollback: back to {v} (restart)"),
            Err(p) => println!("rollback failed: {p}"),
        },
        other => fail(&format!("unknown command {other}")),
    }
    status(&store, &target);
}

fn status(store: &UpdateStore, target: &Path) {
    let l = store.ledger();
    let installed = FakeExe.version_of(target).unwrap_or_else(|| "-".into());
    let aside = FakeExe.version_of(&UpdateStore::aside_path(target)).unwrap_or_else(|| "-".into());
    let v = |c: Option<crate::core::update_store::Copy>| c.map(|c| c.version.to_string()).unwrap_or_else(|| "-".into());
    let skipped = if l.skipped.is_empty() { "-".to_string() } else { l.skipped.join(",") };
    let trial = l.trial.map(|t| format!("{} started={}", t.version, t.started_launches)).unwrap_or_else(|| "-".into());
    println!("status installed={installed} beside={aside} staged={} previous={} kept={} skipped={skipped} trial={trial}", v(store.staged()), v(store.previous()), store.kept());
}
