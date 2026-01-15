---
name: Container-to-Container IPC
overview: Create a new branch for container-to-container IPC testing, building on the existing host-to-container infrastructure. This will enable benchmarking IPC performance between two containers using SharedMemory, TCP, and UDS.
todos:
  - id: create-branch
    content: Create and push container-to-container-ipc branch from host-to-container-ipc
    status: completed
  - id: add-sender-mode
    content: Add Sender run mode to Rust code for C2C sender role
    status: completed
  - id: orchestration-script
    content: Create Python orchestration script for launching and coordinating two containers
    status: completed
    dependencies:
      - create-branch
      - add-sender-mode
  - id: container-setup
    content: Configure shared mounts for /dev/shm and UDS socket volumes
    status: completed
    dependencies:
      - create-branch
  - id: test-shm
    content: Test SharedMemory and SharedMemory_Direct between containers
    status: completed
    dependencies:
      - orchestration-script
      - container-setup
  - id: test-network
    content: Test TCP and UDS between containers
    status: completed
    dependencies:
      - orchestration-script
      - container-setup
  - id: generate-results
    content: Run full benchmark suite and generate CSV report
    status: completed
    dependencies:
      - test-shm
      - test-network
---

# Container-to-Container IPC Testing

## Phase 1: Branch Setup

1. Create new branch `container-to-container-ipc` from current `host-to-container-ipc`
2. Push the new branch to GitHub origin

## Phase 2: Orchestration Infrastructure

Create a Python orchestration script that:

- Launches two containers (sender and receiver)
- Sets up shared volumes for SharedMemory (`/dev/shm`) and UDS socket files
- Coordinates benchmark execution between containers
- Collects and aggregates results

**New file:** `run_container_to_container_tests.py`

## Phase 3: Container Configuration

Modify Containerfile or create a new one that:

- Supports running as either sender or receiver via environment variable
- Mounts shared `/dev/shm` for SharedMemory IPC
- Mounts shared volume for UDS socket files
- Configures container networking for TCP tests

## Phase 4: IPC Mechanism Support

| Mechanism | Required Setup |
|-----------|----------------|
| SharedMemory_Direct | Shared `/dev/shm` mount between containers |
| SharedMemory (Regular) | Shared `/dev/shm` mount between containers |
| TCP Socket | Container network (bridge or host) |
| Unix Domain Socket | Shared volume for socket file |
| POSIX Message Queue | Skip (namespace isolation issues) |

## Phase 5: Testing and Validation

- Run benchmarks across all supported mechanisms
- Compare latency/throughput with host-to-container results
- Generate CSV summary report

---

## Files Created/Modified

| File | Action | Status |
|------|--------|--------|
| `src/cli.rs` | Added `Sender` variant to `RunMode` enum | ✅ Done |
| `src/main.rs` | Added `run_sender_mode_blocking()` function | ✅ Done |
| `run_container_to_container_tests.py` | Created orchestration script | ✅ Done |
| `Containerfile` | No changes needed | ✅ N/A |

## Implementation Notes

### Sender Mode
Added a new `--run-mode sender` that:
- Connects to an existing server (IPC client role)
- Runs the benchmark as the sender
- Collects and outputs results with streaming support
- Sends shutdown message when complete

### Container Architecture
```
Container A (Server)                 Container B (Sender)
┌─────────────────────┐             ┌─────────────────────┐
│ --run-mode client   │             │ --run-mode sender   │
│ (IPC server role)   │◄───────────►│ (IPC client role)   │
│                     │   shared    │                     │
│ Creates resources   │   /dev/shm  │ Connects to server  │
│ Waits for messages  │   or volume │ Sends messages      │
│ Responds to reqs    │             │ Collects metrics    │
└─────────────────────┘             └─────────────────────┘
```

### Verified Results (Container-to-Container)
| Mechanism | Size | Latency Type | Mean Latency |
|-----------|------|--------------|--------------|
| SharedMemory | 64B | one-way | 1.25 µs |
| SharedMemory (direct) | 64B | one-way | 6.60 µs |
| TCP Socket | 64B | round-trip | 63.12 µs |
| Unix Domain Socket | 64B | round-trip | 31.39 µs |