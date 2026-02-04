# Documentation Audit Report

**Date:** 2026-01-23  
**Branch:** container-to-container-ipc  
**Auditor:** AI-assisted documentation review

## Executive Summary

This audit compares all existing `.md` documentation files against the current `ipc-benchmark --help` CLI output to identify:
- Outdated/incorrect information
- Missing documentation
- Broken references
- Non-existent CLI flags documented

### Update (post-audit)

Several high-impact issues identified below have since been corrected in the documentation
(notably `README.md`, `CONFIG.md`, and streaming output/schema examples). The sections below
are retained as a historical record of what was found **at the time of the audit**.

---

## Files Audited

| File | Lines | Status |
|------|-------|--------|
| `README.md` | 1102 | **Updated since audit** |
| `CONFIG.md` | 617 | **Updated since audit** |
| `CONTRIBUTING.md` | 587 | Good |
| `SHM_COMPARISON.md` | 492 | Good |
| `utils/dashboard/README.md` | 217 | Good |
| `AGENTS.md` | N/A | Internal (AI guidelines) |
| `.github/pull_request_template.md` | N/A | Good |

---

## Critical Issues (as of audit date)

### 1. Non-Existent CLI Flags Documented

The following flags were documented but **did not exist** in the CLI at the time
of this audit. Some of these have since been removed from the docs; this table
is preserved to explain what was found during the audit.

| Documented Flag | Found In | Issue |
|-----------------|----------|-------|
| `--config` | CONFIG.md (lines 72-127) | Configuration file feature does not exist |
| `--streaming-output` (documented-but-nonexistent) | CONFIG.md (line 42) | Should be `--streaming-output-json` or `--streaming-output-csv` |
| `--no-one-way` | README.md (line 430), CONFIG.md (line 364) | Flag does not exist |
| `--no-round-trip` | CONFIG.md (line 385) | Flag does not exist |
| `--mechanism` | README.md (lines 929, 935, 957, 966-967, etc.) | Should be `-m` |
| `--dry-run` | CONFIG.md (line 598) | Flag does not exist |

### 2. Missing Referenced Files

| Referenced File | Found In | Status |
|-----------------|----------|--------|
| `CHANGELOG.md` | README.md (line 1035) | Was missing at audit time (now present at `docs/CHANGELOG.md`) |
| `docs/PODMAN_SETUP.md` | Referenced historically | **Deleted** |
| `docs/HOST_CONTAINER_USAGE.md` | Referenced historically | **Deleted** |
| `Containerfile` | CI workflow mentions | Verify current repo state (present in current tree) |

### 3. Incorrect Default Values

| Option | Documented Default | Actual Default |
|--------|-------------------|----------------|
| `--buffer-size` | `8192` (CONFIG.md line 36) | Smart default calculated; PMQ uses 8192 |
| `--one-way` | `true` (CONFIG.md line 32) | Flag (no default; if neither specified, both run) |
| `--round-trip` | `true` (CONFIG.md line 33) | Flag (no default; if neither specified, both run) |

---

## README.md Detailed Issues

### Section: Dashboard Integration (lines 912-1016)

**Issue:** Uses `--mechanism` instead of `-m`

```bash
# INCORRECT (documented)
./ipc-benchmark --mechanism SharedMemory --message-size 1024

# CORRECT (actual CLI)
./ipc-benchmark -m shm --message-size 1024
```

### Section: Advanced Configuration (line 430)

**Issue:** Documents `--no-one-way` which doesn't exist

```bash
# INCORRECT (documented)
ipc-benchmark --round-trip --no-one-way

# CORRECT (how to run only round-trip)
ipc-benchmark --round-trip
# (without --one-way, and without both flags = both tests run)
```

### Section: CI/CD (line 904)

**Issue:** Mentions "Docker Build" validation, but Containerfile was deleted

```
- **Docker Build**: Validates that the Docker image can be built and run successfully.
```

This should be removed or updated.

### Section: Changelog (line 1035)

**Issue:** References non-existent file

```markdown
See [CHANGELOG.md](CHANGELOG.md) for version history and changes.
```

---

## CONFIG.md Detailed Issues

### Section: Configuration File (lines 72-127)

**Critical:** Entire section documents a `--config` flag that does not exist in the current CLI.

```bash
# DOCUMENTED BUT NON-EXISTENT
ipc-benchmark --config config.json
```

**Action Required:** Remove this section or mark it as "Planned Feature"

### Section: Advanced Options (line 42)

**Issue:** Documented a non-existent `--streaming-output` flag instead of the actual flags

```
| `--streaming-output` (non-existent) | String | - | File for streaming results during execution |
```

**Correct:** Should document both:
- `--streaming-output-json [<FILE>]`
- `--streaming-output-csv [<FILE>]`

### Section: Test Scenarios (lines 363-364, 384-385)

**Issue:** Uses non-existent flags

```bash
# INCORRECT
--round-trip \
--no-one-way

# INCORRECT  
--one-way \
--no-round-trip
```

---

## Current CLI Reference (Source of Truth)

### All Current CLI Options

```
Options:
  -m <MECHANISMS>...          [default: uds] [values: uds, shm, tcp, pmq, all]
  -s, --message-size          [default: 1024]
      --one-way               (flag)
      --round-trip            (flag)
  -h, --help
  -V, --version

Timing:
  -i, --msg-count             [default: 10000]
  -d, --duration
      --send-delay
  -w, --warmup-iterations     [default: 1000]

Concurrency:
  -c, --concurrency           [default: 1]
      --server-affinity <CORE>
      --client-affinity <CORE>

Output and Logging:
  -o, --output-file [<FILE>]
  -q, --quiet
  -v, --verbose...
      --log-file <PATH | stderr>
      --streaming-output-json [<FILE>]
      --streaming-output-csv [<FILE>]

Advanced:
      --continue-on-error
      --percentiles           [default: 50 95 99 99.9]
      --buffer-size
      --host                  [default: 127.0.0.1]
      --port                  [default: 8080]
      --pmq-priority          [default: 0]
      --include-first-message
      --blocking
      --shm-direct
      --cross-container
      --run-mode              [default: standalone] [values: standalone, client, sender]
```

---

## Missing Documentation

### Features Not Documented (or Under-documented)

1. **`--cross-container` flag** - Only briefly mentioned, needs explanation
2. **`--run-mode` modes** - client/sender modes need detailed usage guides
3. **`--send-delay`** - Documented in README but not in CONFIG.md
4. **`--pmq-priority`** - Not in CONFIG.md tables
5. **`--include-first-message`** - Not in CONFIG.md tables

### Missing Files

1. **CHANGELOG.md** - Referenced but doesn't exist
2. **Utility scripts documentation** - `fullrun.py`, `generate_summary_csv.py` undocumented

---

## Recommendations

### Immediate Actions (Stage 3)

1. **Remove** references to `--config` flag (doesn't exist)
2. **Remove** references to `--no-one-way` and `--no-round-trip` (don't exist)
3. **Fix** `--streaming-output` (non-existent) → `--streaming-output-json`/`--streaming-output-csv`
4. **Fix** `--mechanism` → `-m` throughout Dashboard section
5. **Remove** "Docker Build" from CI/CD section
6. **Create** `CHANGELOG.md` placeholder
7. **Remove** references to deleted docs (PODMAN_SETUP.md, HOST_CONTAINER_USAGE.md)

### Documentation Restructure (Stage 2+)

1. Create `docs/user-guide/` with focused chapters
2. Create `docs/reference/cli-reference.md` from actual `--help` output
3. Consolidate troubleshooting into dedicated guide
4. Document utility scripts

---

## Verification Commands

To regenerate CLI reference:
```bash
cargo run --release -- --help > docs/reference/cli-help.txt
```

To check for broken links:
```bash
# Install markdown-link-check
npm install -g markdown-link-check
markdown-link-check README.md
```

---

**End of Audit Report**
