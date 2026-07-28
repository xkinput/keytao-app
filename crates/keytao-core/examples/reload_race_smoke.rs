//! Manual smoke test for the librime thread model.
//!
//! Requires a user directory whose schemas are already deployed:
//!
//! ```text
//! reload_race_smoke <user-dir> <shared-dir> [seconds]
//! ```
//!
//! Several threads type on their own sessions while another thread keeps
//! finalizing and re-initializing librime. librime itself is not thread-safe,
//! so this only stays alive as long as `keytao-core` serializes every rime_api
//! call and drains live engines before a reload.

use keytao_core::ImeRuntime;
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn usage() -> String {
    "usage: reload_race_smoke <user-dir> <shared-dir> [seconds]".into()
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let user_dir = PathBuf::from(args.next().ok_or_else(usage)?);
    let shared_dir = args.next().ok_or_else(usage)?;
    let seconds = match args.next() {
        Some(value) => value.parse::<u64>().map_err(|_| usage())?,
        None => 5,
    };
    if args.next().is_some() {
        return Err(usage());
    }

    let runtime = ImeRuntime::with_dirs(&user_dir, shared_dir);
    runtime.init_without_deploy()?;

    let stop = Arc::new(AtomicBool::new(false));
    let keys = Arc::new(AtomicU64::new(0));
    let reloads = Arc::new(AtomicU64::new(0));
    let mut workers = Vec::new();

    for _ in 0..3 {
        let runtime = runtime.clone();
        let stop = stop.clone();
        let keys = keys.clone();
        let session = runtime.create_session()?;
        workers.push(std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                for character in "ia".chars() {
                    session.process_key_result(character as u32, 0);
                    keys.fetch_add(1, Ordering::Relaxed);
                }
                session.state();
                session.is_ascii_mode();
                session.reset();
            }
        }));
    }

    let reloader = {
        let runtime = runtime.clone();
        let stop = stop.clone();
        let reloads = reloads.clone();
        std::thread::spawn(move || -> Result<(), String> {
            while !stop.load(Ordering::Relaxed) {
                runtime.reload_without_deploy()?;
                reloads.fetch_add(1, Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(())
        })
    };

    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    stop.store(true, Ordering::Relaxed);

    reloader.join().map_err(|_| "reload thread panicked")??;
    for worker in workers {
        worker.join().map_err(|_| "input thread panicked")?;
    }

    println!(
        "race smoke ok: keys={} reloads={}",
        keys.load(Ordering::Relaxed),
        reloads.load(Ordering::Relaxed)
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
