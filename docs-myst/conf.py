"""
Sphinx configuration file for rusty-comms documentation.

This configuration uses MyST parser to build documentation from
Markdown files in the docs/ directory.
"""

# -- Project information -----------------------------------------------------

project = "rusty-comms"
copyright = "2026, Red Hat Performance Team"
author = "Red Hat Performance Team"
version = "0.1"
release = "0.1.0"

# -- General configuration ---------------------------------------------------

extensions = [
    "myst_parser",
    "sphinx.ext.autodoc",
    "sphinx.ext.viewcode",
    "sphinx_copybutton",
]

# MyST extensions for advanced Markdown features
myst_enable_extensions = [
    "colon_fence",  # ::: fence for admonitions
    "deflist",  # Definition lists
    "tasklist",  # Task lists with checkboxes
    "fieldlist",  # Field lists
    "substitution",  # Substitutions
]

# Source file suffixes
source_suffix = {
    ".rst": "restructuredtext",
    ".md": "markdown",
}

# The master toctree document
master_doc = "index"

# Exclude patterns
exclude_patterns = ["_build", "Thumbs.db", ".DS_Store"]

# -- Options for HTML output -------------------------------------------------

html_theme = "sphinx_book_theme"

html_theme_options = {
    "repository_url": "https://github.com/redhat-performance/rusty-comms",
    "use_repository_button": True,
    "use_edit_page_button": True,
    "path_to_docs": "docs-myst",
}

html_title = "Rusty-Comms Documentation"
html_short_title = "Rusty-Comms"

# -- Options for LaTeX output ------------------------------------------------

latex_documents = [
    (
        master_doc,
        "rusty-comms.tex",
        "Rusty-Comms Documentation",
        "Red Hat Performance Team",
        "manual",
    ),
]
