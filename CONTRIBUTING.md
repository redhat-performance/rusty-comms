# Contributing to IPC Benchmark Suite

Thank you for your interest in contributing to the IPC Benchmark Suite! This document provides guidelines for contributing to the project.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Setup](#development-setup)
- [Contribution Guidelines](#contribution-guidelines)
- [Code Style](#code-style)
- [Testing](#testing)
- [Documentation](#documentation)
- [Submitting Changes](#submitting-changes)
- [Review Process](#review-process)

## Code of Conduct

This project adheres to the [Contributor Covenant Code of Conduct](https://www.contributor-covenant.org/). By participating, you are expected to uphold this code. Please report unacceptable behavior to the project maintainers.

### Our Pledge

We pledge to make participation in our project a harassment-free experience for everyone, regardless of:
- Age, body size, disability, ethnicity, gender identity and expression
- Level of experience, education, socio-economic status
- Nationality, personal appearance, race, religion
- Sexual identity and orientation

## Getting Started

### Prerequisites

- **Rust**: 1.70.0 or later (MSRV)
- **Git**: For version control
- **Linux**: Development primarily targets Linux (RHEL 9.6+)
- **Basic knowledge**: Familiarity with Rust, IPC mechanisms, and performance testing

### Finding Issues to Work On

1. **Good First Issues**: Look for issues labeled `good-first-issue`
2. **Help Wanted**: Issues labeled `help-wanted` are open for contributions
3. **Documentation**: Issues labeled `documentation` are great for getting started
4. **Performance**: Issues labeled `performance` require deeper system knowledge

### Communication

- **GitHub Issues**: For bug reports and feature requests
- **Discussions**: For general questions and brainstorming
- **Pull Requests**: For code contributions

## Development Setup

### 1. Fork and Clone

```bash
# Fork the repository on GitHub, then clone your fork
git clone https://github.com/YOUR_USERNAME/ipc-benchmark.git
cd ipc-benchmark

# Add upstream remote
git remote add upstream https://github.com/your-org/ipc-benchmark.git
```

### 2. Set Up Development Environment

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install required tools
cargo install cargo-watch
cargo install cargo-tarpaulin  # For code coverage
cargo install cargo-audit      # For security auditing

# Build the project
cargo build

# Run tests
cargo test
```

### 3. Development Tools

```bash
# Auto-rebuild on file changes
cargo watch -x build

# Run tests on changes
cargo watch -x test

# Format code
cargo fmt

# Check code with clippy
cargo clippy
```

## Contribution Guidelines

### Types of Contributions

1. **Bug Fixes**: Fix existing issues or unexpected behavior
2. **New Features**: Add new IPC mechanisms, metrics, or functionality
3. **Performance Improvements**: Optimize existing code
4. **Documentation**: Improve or add documentation
5. **Testing**: Add or improve test coverage

### Before You Start

1. **Check existing issues**: Ensure your contribution isn't already being worked on
2. **Create an issue**: For significant changes, create an issue first to discuss
3. **Follow the roadmap**: Align contributions with project goals
4. **Consider backwards compatibility**: Avoid breaking existing functionality

### Branch Naming

Use descriptive branch names:
- `feature/add-message-queues` - New features
- `fix/shared-memory-deadlock` - Bug fixes
- `docs/improve-readme` - Documentation
- `perf/optimize-serialization` - Performance improvements

## Code Style

### Rust Style Guidelines

Follow the official Rust style guide and project conventions:

```rust
// Use descriptive names
fn calculate_latency_percentiles(values: &[Duration]) -> Vec<PercentileValue> {
    // Implementation
}

// Document public APIs
/// Calculates latency percentiles from a collection of measurements
/// 
/// # Arguments
/// * `values` - Slice of duration measurements
/// * `percentiles` - Percentiles to calculate (e.g., [50.0, 95.0, 99.0])
/// 
/// # Returns
/// Vector of percentile values sorted by percentile
/// 
/// # Examples
/// ```
/// let values = vec![Duration::from_millis(1), Duration::from_millis(2)];
/// let percentiles = calculate_percentiles(&values, &[50.0, 95.0]);
/// ```
pub fn calculate_percentiles(values: &[Duration], percentiles: &[f64]) -> Vec<PercentileValue> {
    // Implementation
}
```

### Code Formatting

```bash
# Format all code
cargo fmt

# Check formatting
cargo fmt --check

# Configure your editor to format on save
```

### Linting

```bash
# Run clippy for linting
cargo clippy

# Run clippy with all features
cargo clippy --all-features --all-targets

# Fix clippy warnings
cargo clippy --fix
```

### Error Handling

Use `anyhow` for error handling:

```rust
use anyhow::{Result, Context};

fn process_benchmark_results(file_path: &Path) -> Result<BenchmarkResults> {
    let contents = std::fs::read_to_string(file_path)
        .with_context(|| format!("Failed to read file: {}", file_path.display()))?;
    
    let results = serde_json::from_str(&contents)
        .with_context(|| "Failed to parse JSON results")?;
    
    Ok(results)
}
```

## Testing

### Test Structure

```
tests/
├── unit/               # Unit tests
├── integration/        # Integration tests
├── benchmarks/         # Performance benchmarks
└── fixtures/           # Test data files
```

### Writing Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_latency_calculation() {
        let values = vec![1000, 2000, 3000]; // nanoseconds
        let percentiles = calculate_percentiles(&values, &[50.0, 95.0]);
        
        assert_eq!(percentiles.len(), 2);
        assert_eq!(percentiles[0].percentile, 50.0);
        assert_eq!(percentiles[0].value_ns, 2000);
    }
    
    #[tokio::test]
    async fn test_ipc_transport() {
        let mut transport = UnixDomainSocketTransport::new();
        let config = TransportConfig::default();
        
        // Test server startup
        let result = transport.start_server(&config).await;
        assert!(result.is_ok());
        
        // Test cleanup
        transport.close().await.unwrap();
    }
}
```

### Running Tests

```bash
# Run all tests
cargo test

# Run specific test module
cargo test metrics

# Run tests with output
cargo test -- --nocapture

# Run tests with specific pattern
cargo test test_latency

# Run ignored tests
cargo test -- --ignored
```

### Integration Tests

```rust
// tests/integration/ipc_mechanisms.rs
use ipc_benchmark::*;
use tempfile::TempDir;

#[tokio::test]
async fn test_unix_domain_socket_communication() {
    let temp_dir = TempDir::new().unwrap();
    let socket_path = temp_dir.path().join("test.sock");
    
    // Test implementation
}
```

### Performance Tests

```rust
// benches/latency_benchmark.rs
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use ipc_benchmark::metrics::*;

fn benchmark_latency_calculation(c: &mut Criterion) {
    let values: Vec<u64> = (0..10000).collect();
    
    c.bench_function("calculate_percentiles", |b| {
        b.iter(|| {
            calculate_percentiles(black_box(&values), black_box(&[50.0, 95.0, 99.0]))
        })
    });
}

criterion_group!(benches, benchmark_latency_calculation);
criterion_main!(benches);
```

## Documentation

### Code Documentation

- Document all public APIs with rustdoc
- Include examples in documentation
- Explain complex algorithms and data structures
- Document error conditions and edge cases

### User Documentation

- Update README.md for user-facing changes
- Update CONFIG.md for configuration changes
- Add examples for new features
- Update troubleshooting sections

### Documentation Style

```rust
/// Manages IPC transport connections and message passing
/// 
/// The `TransportManager` provides a unified interface for different
/// IPC mechanisms, handling connection setup, message serialization,
/// and error recovery.
/// 
/// # Examples
/// 
/// ```
/// use ipc_benchmark::TransportManager;
/// 
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let manager = TransportManager::new();
///     let transport = manager.create_transport(IpcMechanism::UnixDomainSocket)?;
///     
///     // Use transport...
///     Ok(())
/// }
/// ```
/// 
/// # Performance Considerations
/// 
/// The transport manager maintains connection pools for efficiency.
/// Consider using connection pooling for high-throughput scenarios.
pub struct TransportManager {
    // Implementation
}
```

## Submitting Changes

### Pre-Commit Checks

Before submitting a pull request, please run the following checks locally to ensure that the CI pipeline will pass. These checks validate formatting, code quality, and functionality.

1.  **Format Code**: Ensure the code is formatted according to the project's style.
    ```bash
    cargo fmt --all
    ```

2.  **Run Linter**: Check for common mistakes and style issues with Clippy.
    ```bash
    cargo clippy --all-targets --all-features -- -D warnings
    ```

3.  **Run Tests**: Make sure all tests pass.
    ```bash
    cargo test --verbose --all-features
    ```

### Pull Request Process

1. **Update your fork**:
```bash
git fetch upstream
git checkout main
git merge upstream/main
```

2. **Create a feature branch**:
```bash
git checkout -b feature/add-awesome-feature
```

3. **Make your changes**:
   - Follow code style guidelines
   - Add tests for new functionality
   - Update documentation
   - Ensure all tests pass

4. **Commit your changes**:
```bash
git add .
git commit -m "Add awesome feature

- Implement new IPC mechanism for message queues
- Add comprehensive tests and documentation
- Update configuration options

Fixes #123"
```

5. **Push to your fork**:
```bash
git push origin feature/add-awesome-feature
```

6. **Create a pull request**:
   - Use the PR template
   - Provide clear description
   - Link related issues
   - Add screenshots if applicable

### Commit Message Format

Follow conventional commits:

```
type(scope): short description

Longer description explaining what changed and why.

- List important changes
- Reference issues and PRs
- Note breaking changes

Fixes #123
Closes #456
```

**Types:**
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation changes
- `style`: Code style changes
- `refactor`: Code refactoring
- `test`: Test additions/changes
- `chore`: Maintenance tasks

### Pull Request Template

```markdown
## Description
Brief description of changes

## Type of Change
- [ ] Bug fix
- [ ] New feature
- [ ] Breaking change
- [ ] Documentation update

## Testing
- [ ] Tests pass locally
- [ ] Added tests for new functionality
- [ ] Updated documentation

## Checklist
- [ ] Code follows style guidelines
- [ ] Self-review completed
- [ ] Comments added for complex code
- [ ] Documentation updated
- [ ] No breaking changes (or marked as breaking)
```

## Review Process

### What to Expect

1. **Automated checks**: CI/CD pipeline runs tests and linting
2. **Maintainer review**: Code review by project maintainers
3. **Community feedback**: Input from other contributors
4. **Iteration**: Address feedback and update PR

### Review Criteria

- **Correctness**: Code works as intended
- **Performance**: No significant performance regressions
- **Security**: No security vulnerabilities
- **Maintainability**: Code is readable and well-structured
- **Testing**: Adequate test coverage
- **Documentation**: Clear and comprehensive

### Addressing Feedback

```bash
# Make requested changes
git add .
git commit -m "Address review feedback

- Fix memory leak in shared memory transport
- Add error handling for edge cases
- Improve documentation clarity"

# Push updates
git push origin feature/add-awesome-feature
```

## Development Workflow

### Setting Up Development Environment

```bash
# Install development dependencies
cargo install cargo-watch cargo-tarpaulin cargo-audit

# Set up git hooks
cp scripts/pre-commit .git/hooks/
chmod +x .git/hooks/pre-commit

# Configure editor (VS Code example)
# Install rust-analyzer extension
# Configure format on save
```

### Continuous Integration

The project uses GitHub Actions for CI/CD:

```yaml
# .github/workflows/ci.yml
name: CI

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
    - uses: actions/checkout@v3
    - name: Install Rust
      uses: actions-rs/toolchain@v1
      with:
        toolchain: stable
    - name: Run tests
      run: cargo test --all-features
    - name: Check formatting
      run: cargo fmt --check
    - name: Lint with clippy
      run: cargo clippy -- -D warnings
```

### Performance Considerations

When contributing performance-related changes:

1. **Benchmark before and after**: Use `cargo bench` to measure impact
2. **Profile your code**: Use `perf` or other profiling tools
3. **Consider different scenarios**: Test with various message sizes and concurrency levels
4. **Document performance implications**: Update documentation with performance notes

## Getting Help

### Resources

- **Documentation**: README.md, CONFIG.md, and rustdoc
- **Examples**: `examples/` directory
- **Tests**: Look at existing tests for patterns
- **Issues**: Search existing issues for solutions

### Community

- **GitHub Discussions**: For questions and brainstorming
- **GitHub Issues**: For bug reports and feature requests
- **Code Review**: Learn from feedback on PRs

### Troubleshooting

Common development issues:

1. **Build failures**: Check Rust version and dependencies
2. **Test failures**: Ensure clean environment and proper setup
3. **Linting errors**: Run `cargo clippy --fix`
4. **Formatting issues**: Run `cargo fmt`

## License

By contributing to this project, you agree that your contributions will be licensed under the Apache License 2.0.

---

Thank you for contributing to the IPC Benchmark Suite! Your contributions help make this tool better for everyone. 