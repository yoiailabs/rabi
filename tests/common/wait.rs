#![allow(dead_code)]

use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::process::{Child, Command, ExitStatus, Output};
use std::time::{Duration, Instant};

/// A diagnostic hang guard, never a performance assertion. Oversubscribed hosts
/// can take arbitrarily long to schedule otherwise healthy child processes.
pub fn hang_guard_duration() -> Duration {
    let seconds = match std::env::var("RABI_TEST_HANG_GUARD_SECS") {
        Ok(value) => value
            .parse::<u64>()
            .ok()
            .filter(|seconds| *seconds > 0)
            .expect("RABI_TEST_HANG_GUARD_SECS must be a positive integer number of seconds"),
        Err(std::env::VarError::NotPresent) => 120,
        Err(error) => panic!("invalid RABI_TEST_HANG_GUARD_SECS: {error}"),
    };
    Duration::from_secs(seconds)
}

pub fn hang_guard() -> Instant {
    Instant::now()
        .checked_add(hang_guard_duration())
        .expect("RABI_TEST_HANG_GUARD_SECS exceeds the supported deadline range")
}

/// Capture output while polling a child without pipe backpressure or waiting
/// for EOF from a descendant that inherited the child's output descriptors.
pub struct OutputCapture {
    stdout: File,
    stderr: File,
}

impl OutputCapture {
    pub fn new() -> Self {
        fn anonymous_file() -> File {
            let path =
                std::env::temp_dir().join(format!("rabi-test-output-{}", uuid::Uuid::new_v4()));
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .unwrap();
            // This repository's integration tests run on Unix. Unlinking keeps
            // captures private and cleans up even if a test panics.
            fs::remove_file(path).unwrap();
            file
        }
        Self {
            stdout: anonymous_file(),
            stderr: anonymous_file(),
        }
    }

    pub fn apply<'a>(&self, command: &'a mut Command) -> &'a mut Command {
        command
            .stdout(self.stdout.try_clone().unwrap())
            .stderr(self.stderr.try_clone().unwrap())
    }

    pub fn finish(&mut self, child: &mut Child) -> Output {
        let deadline = hang_guard();
        loop {
            let expired = Instant::now() >= deadline;
            if let Some(status) = child.try_wait().unwrap() {
                return self.output(status);
            }
            if expired {
                let _ = child.kill();
                let _ = child.wait();
                panic!("child process did not exit before the test hang guard");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    pub fn output(&mut self, status: ExitStatus) -> Output {
        fn contents(file: &mut File) -> Vec<u8> {
            // Snapshot the length and use a separate read offset: descendants
            // may still hold this file open after the direct child exits.
            let mut bytes = vec![0; file.metadata().unwrap().len().try_into().unwrap()];
            file.read_exact_at(&mut bytes, 0).unwrap();
            bytes
        }
        Output {
            status,
            stdout: contents(&mut self.stdout),
            stderr: contents(&mut self.stderr),
        }
    }
}
