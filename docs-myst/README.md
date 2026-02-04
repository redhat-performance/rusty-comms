# MyST/Sphinx Documentation

This directory contains the MyST/Sphinx configuration for building the rusty-comms documentation as a static website.

## Setup

```bash
# Create virtual environment
python3 -m venv venv
source venv/bin/activate

# Install dependencies
pip install -r requirements.txt
```

## Building

```bash
# Build HTML documentation
make html

# View the output
open _build/html/index.html
```

## Development

For live reloading during development:

```bash
pip install sphinx-autobuild
make livehtml
```

Then open http://127.0.0.1:8000 in your browser.

## Output Formats

```bash
# HTML (default)
make html

# PDF (requires LaTeX)
make latexpdf

# Single-page HTML
make singlehtml

# ePub
make epub
```

## File Structure

```
docs-myst/
├── conf.py           # Sphinx configuration
├── index.md          # Main documentation index
├── requirements.txt  # Python dependencies
├── Makefile          # Build commands
└── _build/           # Generated output (gitignored)
```

## Source Files

The documentation content comes from the `docs/` directory. This directory only contains the Sphinx configuration for building.
