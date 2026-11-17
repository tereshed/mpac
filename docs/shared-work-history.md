# Native session conversion

**Status:** Isolated Claude ↔ Codex prototype. Product integration is proposed.

MPAC converts a saved conversation into a session that the destination harness owns. An imported Claude Code conversation appears in the native Codex `/resume` picker and opens with its original messages in order. The same applies in reverse for a supported Codex-to-Claude conversion. After import, the destination can be resumed outside MPAC.

## User flow

1. Choose **Convert session…** on a session in MPAC, or select an existing CLI transcript discovered in a configured project folder.
2. Choose the destination harness, its profile, and the project folder or worktree. Preview the destination title, the number of messages to import, and any content the adapter cannot represent. Show the current Git branch without switching it.
3. Choose **Import** or **Import and open**. Both create a new native session ID. **Import and open** opens the destination's native session at its input prompt.
4. In a separate terminal, open the destination's own `/resume` picker and select the import. It remains available after both applications restart.

Import creates a snapshot, with the last complete turn as the default boundary for a live source. The original conversation may continue independently. Repeating an import of the same snapshot into the same destination returns the previous result. A later snapshot can be imported explicitly as a new copy.

## Conversion pipeline

Use a Rust conversion module and directional adapters: **source reader → normalized transcript → destination writer → native verification**. A session interchange model avoids writing a separate translator for every pair of formats. Native formats and compatibility tests remain destination-specific.

The interchange model carries the source identity, snapshot boundary, project/worktree, title, and ordered turns with timestamps, roles, and supported content. Preserve user and assistant messages as distinct native entries. The first release targets all complete text turns plus portable file references. Completed tool actions can be mapped to historical annotations when the destination supports them. Report unsupported content in the preview, including attachments whose bytes or target representation are unavailable.

Create new native IDs using the target format. Derive model, credentials, permissions and environment from the chosen destination profile. Leave source system instructions, private reasoning, raw tool payloads, pending approvals, and live execution state out of the conversion. Use the target's own context-window and compaction behavior when it resumes. Conversion itself sends no prompts and runs no model turns.

### Destination strategies

| Destination | Primary path to validate |
| --- | --- |
| **Claude Code / Z.ai** | Emit a Claude-format transcript under the selected profile and workspace identity. Claude Code documents resuming a transcript by JSONL path. Verify picker discovery independently and confirm resume with a clean Claude process. Z.ai uses the same harness format and its own launch profile. [Claude CLI reference](https://code.claude.com/docs/en/cli-reference) |
| **Codex** | Probe a versioned Codex rollout generated in staging, then ask the native App Server to persist a fork into the target profile. The installed Codex CLI 0.167.0 advertises an experimental rollout `path` on `thread/fork` in its generated schema. Validate that this works for converted content and that the resulting ID appears in a fresh native picker. Evaluate a version-specific writer only if no native registration route works. [Codex App Server](https://learn.chatgpt.com/docs/app-server) |
| **Hermes** | Use the documented `hermes sessions import` for eligible Claude and Codex logs and verify the resulting native ID. Enable by capability detection. The installed v0.13.0 session help does not list this command. [Hermes sessions](https://hermes-agent.nousresearch.com/docs/user-guide/sessions) |
| **Antigravity / AGY** | Google documents importing conversations from the Antigravity App. Validate a supported writer or import interface for foreign sessions before offering AGY as a destination. [AGY resume](https://antigravity.google/docs/cli/commands/resume) |
| **DeepSeek** | Add the adapter after identifying the supported CLI build, transcript format and native picker registration path. |

Initial proof of concept uses synthetic conversations and temporary Claude and Codex profiles. Destination support is gated by the installed CLI version and a passing native compatibility test. If a destination is unsupported, show that in the converter and leave the source available for another target.

## Persistence and recovery

- Stage the converted data on the host running the destination CLI. Use a native registration API where available. A validated file-based adapter may publish only new records and must use atomic writes. Avoid direct writes to a live harness's database or indexes.
- Generate a fresh destination identity and journal an import request before publishing. Use a unique request key based on source snapshot, destination harness, profile and project root. On retry after interruption, verify the recorded destination before creating anything else.
- Record source identity, destination identity, adapter version and coverage in the MPAC workspace database. Keep temporary transcript data private to the host user and clean it up after verification. The source is read-only, and the destination transcript stays in the destination harness's ordinary history store.
- Record the import as successful only after a fresh native listing resolves the destination ID and the native reader confirms the imported message sequence. If a CLI exposes the listing only in its TUI, its adapter needs a picker integration test. Keep registration and opening as separate outcomes so a failed launch does not create another import.

## Acceptance tests

1. Convert **Claude → Codex** and **Codex → Claude** using real CLI versions and synthetic conversations with distinct text in multiple user and assistant turns.
2. With MPAC closed, launch a fresh destination CLI in the selected project. Confirm its native `/resume` picker contains the imported title, select it, and verify the original messages have their roles and order. Resume by exact ID in a separate fresh process.
3. Check that no model call happens during import. In an explicitly authorized smoke test, send a single follow-up after resume and verify that it can use a fact from an earlier imported turn.
4. Test duplicate requests, crashes between staging and registration, malformed and incomplete input, unsupported versions, profile collisions, and missing worktrees. Verify the original history is untouched and existing destination sessions still open.
5. After both directions pass, expose **Convert session…** in MPAC. Add Hermes via its native importer, then expand to AGY and DeepSeek as each registration path is verified.

## Run the prototype

The `mpac-convert-session` Rust binary lives in `mpac-probe`. It passed the macOS smoke with Claude Code 2.1.325 and Codex 0.167.0. The smoke script creates disposable profiles, imports a synthetic dialogue in both directions, and exercises the native resume pickers and exact-ID launch. A loopback endpoint intercepts the next request and verifies that both CLIs send all four imported messages with their original roles and order.

```sh
./scripts/cargo.sh build --offline --locked -p mpac-probe --bin mpac-convert-session
python3 scripts/smoke-session-conversion.py target/debug/mpac-convert-session
```

The smoke requires both CLIs on `PATH` and Python's `pexpect`. The converter preserves complete text turns and reports omitted non-text records and incomplete text. Compacted transcripts and unresolved Claude ancestry are rejected. Project UI, ordinary-profile writes, and the retry journal are still pending.
