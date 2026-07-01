# Tier 1 PR Diffs — Before & After

## PR #137 — Check pthread init return values (Fixes #131)

**File:** `src/ipc/shared_memory_direct.rs` — `RawSharedMessage::init()`

<table>
<tr><th>Before (main)</th><th>After (fix/131-shm-direct-mutex-init-checks)</th></tr>
<tr><td>

```rust
unsafe fn init(&mut self) -> Result<()> {
    use std::mem::MaybeUninit;

    // Initialize mutex attributes with PTHREAD_PROCESS_SHARED
    let mut mutex_attr = MaybeUninit::uninit();
    libc::pthread_mutexattr_init(mutex_attr.as_mut_ptr());
    libc::pthread_mutexattr_setpshared(
        mutex_attr.as_mut_ptr(),
        libc::PTHREAD_PROCESS_SHARED,
    );

    // Initialize the mutex
    let mut mutex = MaybeUninit::uninit();
    libc::pthread_mutex_init(
        mutex.as_mut_ptr(),
        mutex_attr.as_ptr(),
    );
    libc::pthread_mutexattr_destroy(mutex_attr.as_mut_ptr());
    self.mutex = mutex.assume_init();

    // Initialize condition variable with PTHREAD_PROCESS_SHARED
    let mut cond_attr = MaybeUninit::uninit();
    libc::pthread_condattr_init(cond_attr.as_mut_ptr());
    libc::pthread_condattr_setpshared(
        cond_attr.as_mut_ptr(),
        libc::PTHREAD_PROCESS_SHARED,
    );

    let mut cond = MaybeUninit::uninit();
    libc::pthread_cond_init(
        cond.as_mut_ptr(),
        cond_attr.as_ptr(),
    );
    libc::pthread_condattr_destroy(cond_attr.as_mut_ptr());
    self.cond = cond.assume_init();

    // Initialize coordination flags and payload length
    self.ready = 0;
    self.client_ready = 0;
    self.payload_len = 0;

    debug!("RawSharedMessage initialized successfully");
    Ok(())
}
```

</td><td>

```rust
unsafe fn init(&mut self) -> Result<()> {
    use std::mem::MaybeUninit;

    // Initialize mutex attributes with PTHREAD_PROCESS_SHARED
    let mut mutex_attr = MaybeUninit::uninit();
    let ret = libc::pthread_mutexattr_init(mutex_attr.as_mut_ptr());
    if ret != 0 {
        return Err(anyhow!(
            "pthread_mutexattr_init failed with errno {}", ret
        ));
    }
    let ret = libc::pthread_mutexattr_setpshared(
        mutex_attr.as_mut_ptr(),
        libc::PTHREAD_PROCESS_SHARED,
    );
    if ret != 0 {
        libc::pthread_mutexattr_destroy(mutex_attr.as_mut_ptr());
        return Err(anyhow!(
            "pthread_mutexattr_setpshared failed with errno {}", ret
        ));
    }

    // Initialize the mutex with process-shared attribute
    let mut mutex = MaybeUninit::uninit();
    let ret = libc::pthread_mutex_init(
        mutex.as_mut_ptr(), mutex_attr.as_ptr(),
    );
    libc::pthread_mutexattr_destroy(mutex_attr.as_mut_ptr());
    if ret != 0 {
        return Err(anyhow!(
            "pthread_mutex_init failed with errno {}", ret
        ));
    }
    self.mutex = mutex.assume_init();

    // Initialize condition variable attributes
    let mut cond_attr = MaybeUninit::uninit();
    let ret = libc::pthread_condattr_init(cond_attr.as_mut_ptr());
    if ret != 0 {
        libc::pthread_mutex_destroy(&mut self.mutex);
        return Err(anyhow!(
            "pthread_condattr_init failed with errno {}", ret
        ));
    }
    let ret = libc::pthread_condattr_setpshared(
        cond_attr.as_mut_ptr(),
        libc::PTHREAD_PROCESS_SHARED,
    );
    if ret != 0 {
        libc::pthread_condattr_destroy(cond_attr.as_mut_ptr());
        libc::pthread_mutex_destroy(&mut self.mutex);
        return Err(anyhow!(
            "pthread_condattr_setpshared failed with errno {}", ret
        ));
    }

    // Initialize the condition variable
    let mut cond = MaybeUninit::uninit();
    let ret = libc::pthread_cond_init(
        cond.as_mut_ptr(), cond_attr.as_ptr(),
    );
    libc::pthread_condattr_destroy(cond_attr.as_mut_ptr());
    if ret != 0 {
        libc::pthread_mutex_destroy(&mut self.mutex);
        return Err(anyhow!(
            "pthread_cond_init failed with errno {}", ret
        ));
    }
    self.cond = cond.assume_init();

    // Initialize coordination flags and payload length
    self.ready = 0;
    self.client_ready = 0;
    self.payload_len = 0;

    debug!("RawSharedMessage initialized successfully");
    Ok(())
}
```

</td></tr>
</table>

---

## PR #138 — Check server exit status after wait() (Fixes #129)

**Files:** `src/benchmark.rs` (4 sites) + `src/benchmark_blocking.rs` (3 sites)

All 7 call sites follow the same pattern change:

<table>
<tr><th>Before (main)</th><th>After (fix/129-server-exit-status-check)</th></tr>
<tr><td>

```rust
client_transport.close_blocking()?;
server_process
    .wait()
    .context("Server process exited with an error")?;
```

</td><td>

```rust
client_transport.close_blocking()?;
let status = server_process
    .wait()
    .context("Failed to wait for server process")?;
if !status.success() {
    warn!(
        "Server process exited with non-zero status: {}",
        status
    );
}
```

</td></tr>
</table>

---

## PR #139 — Blocking round-trip respects include_first_message (Fixes #122)

**File:** `src/benchmark_blocking.rs` — `run_round_trip_test()` message-count path

<table>
<tr><th>Before (main)</th><th>After (fix/122-blocking-roundtrip-first-message)</th></tr>
<tr><td>

```rust
} else {
    // Message-count based test
    let msg_count = self.config.msg_count.unwrap_or_default();

    // Send canary message if first message should not be included
    if !self.config.include_first_message {
        let canary = Message::new(
            u64::MAX, payload.clone(), MessageType::Request,
        );
        if client_transport.send_blocking(&canary).is_ok() {
            let _ = client_transport.receive_blocking();
        }
    }

    for i in 0..msg_count {
        let send_timestamp_ns =
            crate::results::MessageLatencyRecord::current_timestamp_ns();
        let send_time = Instant::now();
        let message = Message::new(
            i as u64, payload.clone(), MessageType::Request,
        );
        client_transport.send_blocking(&message)?;

        if let Some(delay) = self.config.send_delay {
            std::thread::sleep(delay);
        }

        client_transport.receive_blocking()?;

        let latency = send_time.elapsed();

        // Record latency for all measured messages
        if true {
            // Stream latency if enabled
            if let Some(ref mut manager) = results_manager {
                let record = crate::results::MessageLatencyRecord::new(
                    i as u64,
                    self.mechanism,
                    self.config.message_size,
                    crate::metrics::LatencyType::RoundTrip,
                    latency,
                    send_timestamp_ns,
                );
                let _ = manager.stream_latency_record(&record);
            }

            // Record in metrics collector
            metrics_collector.record_message(
                self.config.message_size, Some(latency),
            )?;
        }
    }
}
```

</td><td>

```rust
} else {
    // Message-count based test — match the async runner's
    // skip-first-message logic: send one extra iteration and
    // discard its latency rather than using a canary message.
    let msg_count = self.config.msg_count.unwrap_or_default();
    let iterations = if self.config.include_first_message {
        msg_count
    } else {
        msg_count + 1
    };

    for i in 0..iterations {
        let send_timestamp_ns =
            crate::results::MessageLatencyRecord::current_timestamp_ns();
        let send_time = Instant::now();
        let message = Message::new(
            i as u64, payload.clone(), MessageType::Request,
        );
        client_transport.send_blocking(&message)?;

        if let Some(delay) = self.config.send_delay {
            std::thread::sleep(delay);
        }

        client_transport.receive_blocking()?;

        let latency = send_time.elapsed();

        if i > 0 || self.config.include_first_message {
            if let Some(ref mut manager) = results_manager {
                let record = crate::results::MessageLatencyRecord::new(
                    i as u64,
                    self.mechanism,
                    self.config.message_size,
                    crate::metrics::LatencyType::RoundTrip,
                    latency,
                    send_timestamp_ns,
                );
                let _ = manager.stream_latency_record(&record);
            }

            metrics_collector.record_message(
                self.config.message_size, Some(latency),
            )?;
        }
    }
}
```

</td></tr>
</table>

---

## PR #140 — Forward duration to spawned server (Fixes #121)

**File:** `src/benchmark_blocking.rs` — `spawn_server_process_with_latency_file()`

<table>
<tr><th>Before (main)</th><th>After (fix/121-blocking-forward-duration)</th></tr>
<tr><td>

```rust
// Add message size and count
cmd.arg("--message-size")
    .arg(self.config.message_size.to_string());
cmd.arg("--msg-count")
    .arg(self.get_msg_count().to_string());
```

</td><td>

```rust
// Add message size
cmd.arg("--message-size")
    .arg(self.config.message_size.to_string());

// Forward duration or msg-count (duration takes precedence)
if let Some(duration) = self.config.duration {
    cmd.arg("-d")
        .arg(format!("{}s", duration.as_secs_f64()));
} else if let Some(count) = self.config.msg_count {
    cmd.arg("--msg-count").arg(count.to_string());
}
```

</td></tr>
</table>

---

## PR #141 — Forward missing CLI flags to async server (Fixes #124)

**File:** `src/benchmark.rs` — `spawn_server_process()`

<table>
<tr><th>Before (main)</th><th>After (fix/124-async-server-flag-forwarding)</th></tr>
<tr><td>

```rust
    IpcMechanism::All => {}
}

// Add latency file path if provided
if let Some(path) = latency_file_path {
    cmd.arg("--internal-latency-file").arg(path);
}

let child = cmd.spawn()
    .context("Failed to spawn server process")?;

Ok((child, reader))
```

</td><td>

```rust
    IpcMechanism::All => {}
}

// Forward buffer size so server uses the same transport config
cmd.arg("--buffer-size")
    .arg(transport_config.buffer_size.to_string());

// Forward --shm-direct flag if enabled
if self.args.shm_direct {
    cmd.arg("--shm-direct");
}

// Forward PMQ priority if applicable
#[cfg(target_os = "linux")]
if self.mechanism == IpcMechanism::PosixMessageQueue {
    cmd.arg("--pmq-priority")
        .arg(self.config.pmq_priority.to_string());
}

// Forward send-delay so server can enable precise timestamps
if let Some(delay) = self.config.send_delay {
    let micros = delay.as_micros();
    cmd.arg("--send-delay").arg(format!("{micros}us"));
}

// Add latency file path if provided
if let Some(path) = latency_file_path {
    cmd.arg("--internal-latency-file").arg(path);
}

// Forward verbose flags to the server for debugging
let verbose_count = self.args.verbose;
for _ in 0..verbose_count {
    cmd.arg("-v");
}

let child = cmd.spawn()
    .context("Failed to spawn server process")?;

Ok((child, reader))
```

</td></tr>
</table>
