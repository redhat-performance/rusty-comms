## PRIMARY MISSION: Portable and reproducible IPC benchmark tests

**The most critical aspects of this project are to create portable and reproducible
IPC benchmark tests where our tooling causes minimal interference that may skew the
results and we can trust results across operating environments, builds, and
hardware.**

When working on this project, always ask: **"Does this change help users generate
quality benchmark results that can be trusted?"**

## Development Guidelines

- All git commits should include the tag "AI-assisted-by: <AI agent model(s)>"
  when any AI agents were used for the development work.
- All git commit messages should be thorough and detailed, and they should
  account for all actual changes, not just the chat context.
- All code should be well-documented and docstrings should be complete.
- The README.md and any other documentation files should be kept up-to-date
  with changes.
- Try to maintain a line length of 88 characters max in all code and markdown
  files.
- External dependencies should be limited to confirmed high-quality and
  actively-maintained projects.
- Always run black and isort before commits that include python code

### Licensing

- **Apache 2.0:** All code contributed to this project will be licensed under the Apache
  License, Version 2.0.

### Code Style & Quality

- **Code Formatting:** The project enforces a strict code style using `cargo fmt` for
  rust code and `black` and `flake8` for python code.
- **Automatic Formatting:** All code should be formatted before being submitted in a
  pull request.
