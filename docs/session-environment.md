# Session environment files

In **New session**, use **Load ENV from a file** to choose an environment file for the CLI. The filename and extension are arbitrary. Browse or search for the file, including hidden files, or enter an absolute path or a path relative to the working folder. The picker lists files without showing their contents.

For an existing Z.ai/GLM setup, choose **Claude Code** and load your prepared environment file, such as `.env.zai`. Agent permissions are configured separately. If you need the equivalent of `--dangerously-skip-permissions`, explicitly choose and confirm that access mode.

MPAC supports UTF-8 with an optional BOM, CRLF, `KEY=value`, `export KEY=value`, comments, quoted and multiline values, and `$NAME` or `${NAME}` substitutions. Substitutions read values already defined in the file, then the daemon's environment. The `${NAME:-fallback}` form is unsupported. MPAC parses the file as data and does not evaluate shell commands.

The file is read once at launch. Only the new CLI and its descendants receive the variables. Session details show the file path and number of variables. Values are excluded from the session database, API responses, and MPAC diagnostic messages.

The chosen file is applied after the usual cleanup of provider API variables for Claude Code and Codex. This lets you set a provider endpoint, credentials, and model for that launch. The CLI and its configuration determine the billing route. MPAC resolves the CLI executable from the daemon's original PATH before applying any PATH from the file.

Environment files are supported for Claude Code, Codex, and Hermes. MPAC reserves `MPAC_EVENT_CHANNEL` and `MPAC_EVENT_PROVIDER`. Use a regular file owned by the current user that other users cannot write. Symlinks, pipes, and device files are rejected. Limits are 256 KiB for the file and combined values, 32 KiB per value, and 256 variables. An explicit `SHELL` must be an absolute executable path.

If the host lacks the `session_env_files` capability, update it before using an environment file. MPAC will not launch a session without a selected file if support disappears while the form is open.
