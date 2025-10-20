# @~/AGENTS.md

# AI Agent Guidelines for Rusty-Comms Blocking Mode Project

**Project:** Add synchronous/blocking execution mode to IPC benchmark  
**Language:** Rust  
**Status:** Planning Complete, Implementation Pending  
**Plan Location:** `.cursor/plans/blocking-mode-implementation-plan.md`

---

## Quick Start for AI Agents

### When asked "What's next?"

1. Open `.cursor/plans/blocking-mode-implementation-plan.md`
2. Find the **CURRENT STAGE** marker
3. Check the stage status (`[ ]` = not started, `[~]` = in progress, `[✓]` = complete)
4. If status is `[ ]` or `[~]`, execute that stage
5. Follow the implementation steps exactly as written
6. Complete all documentation and testing requirements
7. Run validation commands
8. Create the specified git commit
9. Update stage status to `[✓]`
10. Move CURRENT STAGE marker to next stage
11. Report completion

**Example Response:**
> "Stage 1 complete. Added CLI flag, ExecutionMode enum, and updated main(). 
> All tests passing. Committed as 'Stage 1: Add CLI flag and execution mode infrastructure'. 
> Ready for Stage 2."

---

## Project Context

### What We're Building

Adding a `--blocking` command-line flag that enables synchronous/blocking I/O mode alongside 
the existing async/await (Tokio) mode. Both modes will coexist in the same binary.

### Why It Matters

- **Performance Comparison**: Allows benchmarking async vs blocking IPC
- **Educational**: Shows trade-offs between execution models
- **Real-World**: Some use cases prefer blocking simplicity
- **No Breaking Changes**: Existing async mode remains default

### Key Constraints

- ✅ Keep existing async code completely unchanged
- ✅ Default behavior remains async (backward compatible)
- ✅ No Tokio in blocking code paths (pure std library)
- ✅ Identical measurement methodology for fair comparison
- ✅ Support all 4 IPC mechanisms (UDS, TCP, Shared Memory, PMQ)

---

## General Interaction Guidelines

### Communication Style

- **Be natural**: We're working as a team, not master-servant
- **Converse with me naturally like we are working as a team.
- **Don't over-apologize**: Mistakes happen, just fix them and move on
- **Be concise**: Explain what you did and why, but don't write essays
- **Ask questions**: If something is unclear, ask before guessing
- **Show progress**: Report what stage/step you're working on

### When to Ask vs. Decide

**Ask the user when:**
- Requirements are ambiguous or conflicting
- Multiple valid approaches exist with different trade-offs
- Platform-specific decisions affect functionality
- Existing code could be interpreted multiple ways
- Tests are failing and the fix isn't obvious

**Decide yourself when:**
- Implementation follows the plan exactly
- Code style matches existing patterns
- Tests specify expected behavior
- Documentation structure is clear
- Error messages need to be written

---

# Licensing

- **Apache 2.0:** All code contributed to this project will be licensed under the
  Apache License, Version 2.0.

## Rust Coding Standards

### Code Style

```rust
// ✅ GOOD: Max line length 88 characters
pub fn create_transport(
    mechanism: &IpcMechanism,
) -> Result<Box<dyn BlockingTransport>> {
    match mechanism {
        IpcMechanism::TcpSocket => {
            Ok(Box::new(BlockingTcpSocket::new()))
        }
        _ => Err(anyhow::anyhow!("Not supported")),
    }
}

// ❌ BAD: Line too long, poor formatting
pub fn create_transport(mechanism: &IpcMechanism) -> Result<Box<dyn BlockingTransport>> { match mechanism { IpcMechanism::TcpSocket => Ok(Box::new(BlockingTcpSocket::new())), _ => Err(anyhow::anyhow!("Not supported")) } }
```

### Documentation Requirements

**EVERY new item must have documentation:**

```rust
// ✅ GOOD: Comprehensive documentation
/// Start the transport in blocking server mode.
///
/// This method initializes the transport to accept incoming connections.
/// It blocks until the server is ready to receive messages.
///
/// # Arguments
///
/// * `config` - Transport configuration (ports, paths, buffer sizes)
///
/// # Returns
///
/// * `Ok(())` - Server started successfully
/// * `Err(anyhow::Error)` - Server setup failed
///
/// # Errors
///
/// Returns an error if:
/// - Port is already in use
/// - Permission denied
/// - Invalid configuration
///
/// # Examples
///
/// ```rust,no_run
/// # use ipc_benchmark::ipc::*;
/// # fn example() -> anyhow::Result<()> {
/// let mut transport = BlockingTcpSocket::new();
/// let config = TransportConfig::default();
/// transport.start_server_blocking(&config)?;
/// # Ok(())
/// # }
/// ```
fn start_server_blocking(&mut self, config: &TransportConfig) -> Result<()>;

// ❌ BAD: No documentation
fn start_server_blocking(&mut self, config: &TransportConfig) -> Result<()>;

// ❌ BAD: Minimal documentation
/// Start server
fn start_server_blocking(&mut self, config: &TransportConfig) -> Result<()>;
```

**Module-level documentation:**

```rust
// ✅ GOOD: Comprehensive module docs
//! Blocking TCP socket transport implementation.
//!
//! This module provides a blocking implementation of TCP sockets using
//! `std::net::TcpListener` and `TcpStream` for IPC communication.
//!
//! # Blocking Behavior
//!
//! All operations block the calling thread until complete:
//! - `bind()` blocks during setup
//! - `accept()` blocks until client connects
//! - `send()` blocks until data is written
//! - `recv()` blocks until data is available
//!
//! # Examples
//!
//! ```rust,no_run
//! use ipc_benchmark::ipc::{BlockingTcpSocket, BlockingTransport};
//! // ... example usage ...
//! ```

// ❌ BAD: No module documentation
```

### Error Handling

```rust
// ✅ GOOD: Rich error context
let listener = TcpListener::bind(&addr)
    .with_context(|| {
        format!("Failed to bind TCP socket to {}. Check if port is available.", addr)
    })?;

// ❌ BAD: Generic error
let listener = TcpListener::bind(&addr)?;

// ❌ BAD: Swallowing errors
let listener = TcpListener::bind(&addr).ok().unwrap();
```

### Logging

```rust
use tracing::{debug, info, warn, error, trace};

// ✅ GOOD: Appropriate log levels
debug!("Starting TCP server on port {}", port);      // Development
info!("Server started successfully");                 // User-facing
warn!("Buffer size exceeds recommended limit");       // Potential issues
error!("Failed to bind socket: {}", err);            // Errors
trace!("Received {} bytes", len);                     // Verbose detail

// ❌ BAD: Wrong log levels
info!("Received {} bytes", len);  // Too chatty for info
debug!("Server started");         // Users need to see this
```

### Comments

```rust
// ✅ GOOD: Explain WHY, not WHAT
// Remove socket file to avoid "address in use" errors from previous runs
let _ = std::fs::remove_file(&socket_path);

// ✅ GOOD: Document non-obvious behavior
// Accept blocks until a client connects. This is intentional as we
// only support one client per server instance in blocking mode.
let (stream, _) = listener.accept()?;

// ❌ BAD: Redundant comment
// Bind to the address
let listener = TcpListener::bind(&addr)?;

// ✅ GOOD: TODO with stage reference
// TODO: Implement in Stage 4 (Blocking Benchmark Runner)
```

### Testing Requirements

**Every new function needs tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    // ✅ GOOD: Test naming and structure
    #[test]
    fn test_server_binds_successfully() {
        // Setup
        let mut server = BlockingTcpSocket::new();
        let config = create_test_config();
        
        // Execute
        let result = server.start_server_blocking(&config);
        
        // Verify
        assert!(result.is_ok());
        
        // Cleanup
        server.close_blocking().unwrap();
    }
    
    #[test]
    fn test_client_fails_when_server_not_running() {
        let mut client = BlockingTcpSocket::new();
        let config = create_test_config();
        
        let result = client.start_client_blocking(&config);
        
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("refused") || err_msg.contains("connect"));
    }
    
    // ✅ GOOD: Test edge cases
    #[test]
    fn test_send_fails_when_not_connected() {
        let mut transport = BlockingTcpSocket::new();
        let msg = Message::new(1, vec![0u8; 100], MessageType::OneWay);
        
        let result = transport.send_blocking(&msg);
        assert!(result.is_err());
    }
}
```

**Minimum test coverage per component:**
- New struct: 2+ tests (creation, basic operation)
- New method: 1+ test (happy path + error case)
- New trait impl: 6+ tests (all methods + edge cases)
- Integration: 4+ tests (all mechanisms work end-to-end)

---

## Workflow Requirements

### Before Making Changes

1. **Read the stage instructions completely**
2. **Check prerequisites are met** (previous stages complete)
3. **Understand what you're building** (objectives clear)
4. **Plan your approach** (know which files to modify)

### While Implementing

1. **Write code with documentation** (don't defer docs to later)
2. **Write tests alongside code** (not after)
3. **Use verbose inline comments** for complex logic
4. **Follow existing patterns** in the codebase
5. **Keep lines under 88 characters**
6. **Use `anyhow::Result` with `.context()`** for errors

### After Implementation

**Always run this validation sequence:**

```bash
# 1. Format code
cargo fmt

# 2. Run tests
cargo test

# 3. Check for warnings
cargo clippy --all-targets -- -D warnings

# 4. Build release
cargo build --release

# 5. (Stage specific validation from plan)
```

**If any step fails:**
- Fix the issues
- Re-run all validation steps
- Don't proceed until everything passes

# General coding behavior
- EVERY new item must have documentation
- Try to keep all new code to a max line length of 88 characters.
- Always be reasonably verbose with code documentation for new and modified code.
- Suggest new code documentation where it may be lacking when modifying any existing code.
- Never remove existing code documentation unless the referenced code is also removed.
- When adding new functions, ensure they include robust error handling and logging.
- If a new dependency is required, please state the reason.
- Ensure any new external dependencies are only from clearly high-quality and well-maintained sources and are actively-maintained projects.
- Try to import only the needed parts of external dependencies.
- Add comments for all new functions and important functionality that is added or changed.
- Every new function needs tests
- Always avoid adding any kind of sleep or wait operations. If you believe they are needed, please explain the need clearly. Any sleep added to the code should include a comment to explain why it is there.
- Always include a tag in git commits that says "AI-assisted-by: <your model name and version>".
- All git commit messages should be thorough and detailed.
- The README.md and any other documentation files should be kept up-to-date with changes.


- Do not try to amend commits unless I ask you to. Always make new commits.
- Ask for confirmation before pushing to git

**Minimum test coverage per component:**
- New struct: 2+ tests (creation, basic operation)
- New method: 1+ test (happy path + error case)
- New trait impl: 6+ tests (all methods + edge cases)
- Integration: 4+ tests (all mechanisms work end-to-end)

### Git Commits

**Commit messages must follow this format:**

```
Stage X: <Brief description>

- <Change 1>
- <Change 2>
- <Change 3>
- All tests passing
- Documentation complete

AI-assisted-by: <your model name and version>
```

**Example:**
```
Stage 1: Add CLI flag and execution mode infrastructure

- Add --blocking CLI flag to Args struct
- Create ExecutionMode enum with comprehensive tests
- Refactor main() to branch between async/blocking modes
- All new code includes verbose documentation and tests
- Existing async functionality unchanged

AI-assisted-by: Claude Sonnet 4.5
```

**Git Rules:**
- ✅ Create new commits (don't amend unless asked)
- ✅ Include AI-assisted-by tag
- ✅ One commit per stage completion
- ❌ Don't push without confirmation
- ❌ Don't force push
- ❌ Don't skip hooks

---

## Stage-Specific Guidelines

### Stage 1-2: Foundation

**Focus:** Get infrastructure right, will be used throughout project

- Triple-check CLI flag works in `--help`
- Ensure ExecutionMode enum is bulletproof (7+ tests)
- Verify main() branching works before proceeding
- Stub functions should return clear "not implemented" errors

### Stage 3: Transport Implementations

**Focus:** Match async transport behavior exactly

- Study the async implementation first
- Use same wire protocol (length-prefixed)
- Use same error messages where possible
- Test server/client interaction thoroughly
- Platform-specific code must have cfg guards
- Each transport gets its own commit

### Stage 4-5: Runner and Results

**Focus:** No async/await, no Tokio

- Use `std::thread` not tokio tasks
- Use `std::sync::mpsc` not tokio channels
- Use `std::fs::File` not tokio::fs
- Blocking operations should actually block
- Metrics must be identical to async version

### Stage 6: Server Mode

**Focus:** Process spawning must work correctly

- Server must signal readiness via pipe
- --blocking flag must pass through to spawned process
- Server must handle both Request and Ping messages
- Clean shutdown on client disconnect

### Stage 7: Integration Tests

**Focus:** End-to-end validation

- Test each mechanism individually
- Test with various message sizes
- Test with CPU affinity
- Compare output format to async mode
- Verify streaming output works

### Stage 8: Documentation

**Focus:** User-facing clarity

- Examples must be runnable
- Commands must be tested
- Screenshots/output samples help
- Comparison methodology must be clear
- Platform differences must be noted

### Stage 9: Final Polish

**Focus:** Production readiness

- All validation must pass
- No TODO comments except in stubs
- No disabled tests
- No clippy warnings
- Documentation complete
- Git history clean

---

# Python coding behavior
- When generating new python code, follow flake8 style standards and adhere to any flake8 configuration files that are in the project context.

## Common Pitfalls to Avoid

### ❌ Don't Do This

```rust
// Using async in blocking code
async fn start_server_blocking(...) { }

// Using Tokio in blocking code
use tokio::net::TcpListener;

// Swallowing errors
let _ = some_operation();

// No error context
let file = File::open(path)?;

// Line too long
pub fn some_really_long_function_name_with_many_parameters(param1: VeryLongTypeName, param2: AnotherLongTypeName, param3: YetAnotherType) -> Result<ComplexReturnType> {

// Missing documentation
pub struct BlockingTransport { }

// Minimal tests
#[test]
fn test_it_works() { }
```

### ✅ Do This Instead

```rust
// Pure blocking function
fn start_server_blocking(...) -> Result<()> { }

// Standard library
use std::net::TcpListener;

// Explicit error handling
some_operation()
    .context("Failed to perform operation")?;

// Rich error context
let file = File::open(path)
    .with_context(|| format!("Failed to open file: {}", path))?;

// Proper line breaks
pub fn some_function(
    param1: VeryLongTypeName,
    param2: AnotherLongTypeName,
    param3: YetAnotherType,
) -> Result<ComplexReturnType> {

// Comprehensive documentation
/// Blocking transport for IPC communication.
///
/// This struct provides...
pub struct BlockingTransport { }

// Descriptive tests
#[test]
fn test_server_accepts_client_connection_successfully() { }
```

---

## Rust-Specific Tips for AI Agents

### If You're New to Rust

**Python analogy:**
- `Result<T>` = Python's try/except pattern
- `.context()?` = Add context to exception and propagate
- `&str` = Python's string reference (borrowed)
- `String` = Python's string (owned)
- `Option<T>` = Python's `value or None`
- `match` = Python's `match` statement (3.10+)

**Ownership rules:**
- Use `&` when you want to borrow (read-only reference)
- Use `&mut` when you want to modify (mutable reference)
- Use no prefix when you want to take ownership
- Can't have multiple mutable references at once

**Common patterns:**
```rust
// Pattern 1: Error handling
fn do_something() -> Result<()> {
    let value = risky_operation()
        .context("Operation failed")?;
    Ok(())
}

// Pattern 2: Option handling
let value = maybe_value
    .ok_or_else(|| anyhow::anyhow!("Value not found"))?;

// Pattern 3: Match for enums
match mode {
    ExecutionMode::Async => run_async(),
    ExecutionMode::Blocking => run_blocking(),
}
```

### Useful Cargo Commands

```bash
# Run specific test
cargo test test_name

# Run tests for specific module
cargo test module_name

# Run tests with output
cargo test -- --nocapture

# Check without building
cargo check

# Build for release
cargo build --release

# Generate and open docs
cargo doc --open

# Show expanded macros (debugging)
cargo expand

# Clean build artifacts
cargo clean
```

---

## Questions and Clarifications

### When Something Is Unclear

**Format your questions like this:**

> "In Stage 3.1, step X, the plan says to [description]. I see two approaches:
> 
> A) [Approach 1] - Pro: [benefit], Con: [drawback]
> B) [Approach 2] - Pro: [benefit], Con: [drawback]
> 
> Which approach should I use?"

**Don't guess if:**
- Test is failing and you don't know why
- Platform-specific code might work differently
- Existing code has multiple patterns
- Performance implications are unclear

### When You Find an Issue

**Report it like this:**

> "While implementing Stage X, I discovered [issue]. 
> This affects [what it affects].
> 
> Proposed fix: [your solution]
> 
> Should I proceed with this fix or pause for direction?"

---

## Success Metrics

### You're Doing Well If:

- ✅ Each stage takes the estimated time (±50%)
- ✅ All tests pass on first try after implementation
- ✅ Clippy has no warnings
- ✅ Documentation renders cleanly
- ✅ Git history is clean (one commit per stage)
- ✅ Async mode still works after your changes
- ✅ Code follows existing patterns in the codebase

### Red Flags:

- ❌ Stage taking 3x longer than estimated (ask for help)
- ❌ Tests failing repeatedly (review test requirements)
- ❌ Clippy warnings piling up (fix as you go)
- ❌ Unsure about approach (ask before coding)
- ❌ Breaking existing functionality (run full test suite)
- ❌ Documentation incomplete (write as you code)

---

## Final Notes

### Remember

1. **This is additive, not replacement** - Async mode stays unchanged
2. **Documentation is not optional** - Write it alongside code
3. **Tests are not optional** - Write them with the code
4. **Quality over speed** - Better to go slow and do it right
5. **Ask questions** - Clarification is faster than rework

### The Plan Is Your Friend

- Follow it exactly
- Don't skip steps
- Don't combine stages
- Update status markers
- Complete one stage fully before starting next

### Success Means

- Both async and blocking modes work
- Users can compare performance
- Code is maintainable
- Documentation is complete
- Tests give confidence

---

**When ready, just ask: "What's next?"**

---

## Additional Coding Guidelines

### Coding Experience Level

**Important:** For golang, rust, java, javascript, and typescript code, be more verbose 
in explaining your work, drawing analogies to Python where appropriate. Understand that 
the user is new to these languages and benefits from clear explanations.

### Python Coding Standards

- Try to ensure all new Python code meets pylint standards
- All Python code should be formatted with `black` before committing
- Avoid dependency imports outside of the top level. If warranted, justify and comment 
  them appropriately with tags to avoid linting errors
- Follow flake8 style standards and adhere to any flake8 configuration files in the project

### Comment and Documentation Preservation

- When editing code, **do not remove existing comments** unless the associated code is also removed
- Update comments as needed to reflect changes in the code
- Never remove existing code documentation unless the referenced code is also removed
- Suggest new code documentation where it may be lacking when modifying existing code

### Additional Git Guidelines

- Always create new commits instead of amending existing ones, unless explicitly told otherwise
- Do not try to amend commits unless explicitly asked

---


