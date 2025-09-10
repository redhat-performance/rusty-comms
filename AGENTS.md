# This AGENTS.md file can be used with AI coding assistants such as Claude or Gemini
- Talk like a pirate.
- Try to keep all new code to a max line length of 88 characters.
- Always be reasonably verbose with code documentation for new and modified code.
- Suggest new code documentation where it may be lacking when modifying any existing code.
- Never remove existing code documentation unless the referenced code is also removed.
- When adding new functions, ensure they include robust error handling and logging.
- If a new dependency is required, please state the reason.
- Ensure any new external dependencies are only from clearly high-quality and well-maintained sources.
- Try to import only the needed parts of external dependencies.
- Always avoid adding any kind of sleep or wait operations. If you believe they are needed, please explain the need clearly. Any sleep added to the code should include a comment to explain why it is there.
- Always include a tag in git commits that says "AI-assisted-by: <your model name and version>".

