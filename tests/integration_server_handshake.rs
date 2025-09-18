use os_pipe::pipe;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::io::{FromRawFd, IntoRawFd};
#[cfg(windows)]
use std::os::windows::io::{FromRawHandle, IntoRawHandle};
use std::process::{Command, Stdio};
use std::time::Duration;

#[test]
fn server_ready_handshake_via_stdout_pipe() {
    // Create an OS pipe: parent will read from `reader`.
    let (mut reader, writer) = pipe().expect("create pipe");

    // Spawn a short-lived child that writes a single byte to stdout then exits.
    // The child stdout is connected to the writer end of our pipe.
    #[cfg(unix)]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.arg("-c").arg("printf '\\001'; sleep 0.05");
        c
    };
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        // Use a simple echo to write a deterministic single-byte 'R' to stdout
        c.arg("/C").arg("echo R");
        c
    };
    // attach stdio
    cmd.stdin(Stdio::null());
    // attach the writer end of our pipe to the child's stdout
    #[cfg(unix)]
    {
        cmd.stdout(unsafe { Stdio::from_raw_fd(writer.into_raw_fd()) });
    }
    #[cfg(windows)]
    {
        cmd.stdout(unsafe { Stdio::from_raw_handle(writer.into_raw_handle()) });
    }
    cmd.stderr(Stdio::null());

    let mut child = cmd.spawn().expect("spawn child");

    // Parent reads a single byte from the pipe reader to observe the ready signal.
    let mut buf = [0u8; 1];

    // allow a small timeout for the child to write
    let start = std::time::Instant::now();
    loop {
        match reader.read_exact(&mut buf) {
            Ok(_) => break,
            Err(_) if start.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(e) => panic!("failed reading ready byte from pipe: {}", e),
        }
    }

    // ensure the child wrote the expected ready signal
    #[cfg(unix)]
    assert_eq!(buf[0], 1u8, "expected ready byte 0x01 from child stdout");
    #[cfg(windows)]
    assert_eq!(buf[0], b'R', "expected 'R' from child stdout");

    // reap child
    let _ = child.wait();
}
