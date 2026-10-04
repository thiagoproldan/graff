1. Every command that sets a task description can take the description from stdin instead of an argument. This means task creation at minimum, plus editing if that command accepts a description.
2. All of stdin becomes the description exactly as written. Every line up to EOF is read, and apostrophes, double quotes, backslashes, `$`, backticks and inner line breaks stay intact. Nothing is escaped or stripped, though trimming the trailing newline that a heredoc or echo adds is fine.
3. Existing usage still works:
   - A description passed as an argument behaves as before.
   - Stdin is read only when a command asks for it. Nothing hangs or swallows input when stdin is not a terminal, such as in an agent's shell or a script loop.
   - MCP server mode's stdin is left alone.
4. `--help`, and any agent-facing docs or instructions ekko ships, describe the stdin form. A quoted-heredoc example is ideal, so agents can keep apostrophes without fighting shell quoting.
5. Tests pipe text containing apostrophes, quotes and several lines through stdin, then check that the stored description matches it exactly.
6. Failures are handled cleanly. Unreadable or non-UTF-8 stdin gives a normal CLI error, not a panic. If a description can be given both as an argument and via stdin at once, that case is rejected or follows a clear precedence.
7. How stdin is requested: `-` as the description value, a dedicated flag, a file option that accepts `-`, or something similar (open).
8. Whether other long-text arguments besides the description also get a stdin option (open).
