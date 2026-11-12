use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Stdio};

/// Write executable fixtures in a child so parallel forks in the test process
/// cannot inherit a writable script descriptor and cause Linux ETXTBSY.
pub fn write_executable(path: impl AsRef<Path>, body: impl AsRef<[u8]>) -> io::Result<()> {
    let path = path.as_ref();
    let result = (|| {
        let mut child = Command::new("/bin/sh")
            .args([
                "-c",
                "umask 077; /bin/cat > \"$1\" && /bin/chmod 700 \"$1\"",
                "sh",
            ])
            .arg(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        let written = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("script writer stdin is unavailable"))
            .and_then(|mut stdin| stdin.write_all(body.as_ref()));
        // Close stdin and reap the writer even if sending the body failed.
        let output = child.wait_with_output()?;
        written?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "script writer exited with {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    })();
    result.map_err(|error: io::Error| {
        io::Error::new(
            error.kind(),
            format!("write executable fixture {}: {error}", path.display()),
        )
    })
}
