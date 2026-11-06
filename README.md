# agent-office

English | [简体中文](README.zh-CN.md)

A pixel office on Victoria Harbour for your Claude Code sessions. Every session is a little worker with its current task over its head. See at a glance who is waiting for your approval and who looks stuck, and double-click to jump back to that session.

![agent-office](docs/demo.gif)

## What it does

With several agent sessions running at once, it's hard to keep track of which one finished, which one is waiting for you to click "Allow", and which one has been going in circles for ten minutes. agent-office puts them all in one office:

- **One-line summaries**: reads each session's latest prompt, todos and recent actions, and summarizes them in one short line through an OpenAI-compatible API you configure. When a turn ends it summarizes the result too: what got done, where it got stuck, or what it is asking you.
- **Seats by state**: working sessions sit at their desks, finished ones go to the pantry, sessions waiting for approval or asking you a question queue in the waiting area, and stuck ones stay at their desk with a red "?". The side panel groups them as waiting → stuck → done, unread → working → idle.
- **Stuck detection**: 3 failures in a row, or no new activity for 5 minutes while working (20 minutes while a shell command or subagent is running), marks a session as possibly stuck.
- **Notifications**: when the window is in the background and a session starts waiting for you, you get a system notification. The Dock badge always shows how many are waiting.
- **Jump back**: double-click a worker or card to open that session in the Claude desktop app, or bring forward the terminal it runs in.
- **Several agents**: Claude Code, Codex, Cursor (`cursor-agent`) and DeepSeek Harness (dsh).

The UI comes in English and Simplified Chinese, following your macOS language; press `l` in the window to switch (saved for the next start), or set `AGENT_OFFICE_LANG`.

## Requirements

- macOS (the only platform it has been used on)
- Rust 1.95 or newer, only when building from source
- Optional: an OpenAI-compatible chat completions endpoint for summaries. Without one it still works, and the panel shows each session's raw prompt.

## Quick start

```bash
brew install tyler-y-liu/tap/agent-office
agent-office connect      # adds hooks to ~/.claude/settings.json (backs it up first); applies to new Claude sessions
agent-office
```

You can also build from source with `cargo install --git https://github.com/tyler-y-liu/agent-office`. Each [release](https://github.com/tyler-y-liu/agent-office/releases) also has a universal binary for Apple Silicon and Intel.

To turn on summaries, put these in `~/.config/agent-office/env` (or set them as environment variables):

```bash
AGENT_OFFICE_API_BASE=https://your-gateway.example.com
AGENT_OFFICE_API_KEY=sk-...
AGENT_OFFICE_MODEL=your-model
```

Requests go to `<base>/v1/chat/completions`, or to `<base>/chat/completions` when the base already names an API version (`/v1`, `/api/v3`, `/v1beta/openai`, ...). Pick a fast model without long reasoning.

If something looks wrong, run `agent-office doctor`. It checks the session directories, the hooks for each agent, the socket and the stuck thresholds, and makes one test call to the summary endpoint.

It also works without hooks, from session transcripts alone, but then it can't tell when a session is waiting for your approval.

### Other agents

| Agent | Setup | Notes |
|---|---|---|
| Codex | Shows up without setup; run `agent-office connect codex` to see approval waits | Reads `$CODEX_HOME/sessions` (default `~/.codex/sessions`). `connect` puts a plugin in `~/.config/agent-office/codex-plugin` and installs it with `codex plugin`; needs `codex` on PATH |
| Cursor (`cursor-agent`) | `agent-office connect cursor` | Adds hooks to `hooks.json` in the Cursor config directory (default `~/.cursor`; backs it up first). Cursor's own transcripts have no tool calls, so the hook keeps a task log in `~/.config/agent-office/cursor/transcripts/` |
| DeepSeek Harness (dsh) | `agent-office connect dsh` | Writes a plugin to `$DSH_HOME` (default `~/.dsh`) and adds it to `cordis.patch.yml`. The plugin keeps a task log in `$DSH_HOME/agent-office/transcripts/` |

Task logs hold only your prompts, the agent's replies, todos, and the path or command of each tool call. They contain no file contents or tool output. A log untouched for 30 days is deleted.

Setup applies to sessions started afterwards. `agent-office disconnect [codex|cursor|dsh]` removes it and keeps the logs. Hooks point at wherever the binary was when you ran `connect`, so run `connect` again after moving or reinstalling it.

## Usage

### The office

A row of four-person desks runs under the north windows, with a corridor in the middle. Along the south side, west to east: meeting room A, meeting room B, the pantry, the waiting area, and your office. A wide window adds a rooftop terrace at the west end. Click a door sign to see what that area is for.

| Where | Who |
|---|---|
| Desks | Working sessions. Sessions from the same repo share a desk row, with matching colours on their name tags and panel avatars |
| Meeting rooms | A session whose subagent has run for 15 seconds goes there with it |
| Waiting area | Waiting for approval for over 20 seconds, or finished and asking you a question. Approval waits go first; the first in line takes the visitor chair in your office |
| Pantry | Finished, waiting for you to look |
| Rooftop | You've opened it since it finished, or it finished over 15 minutes ago |

The rooftop appears only when the office is at least 200 columns wide. When the desks fill up, a new floor opens; switch with `[` and `]`.

### Keys

| Key | Action |
|---|---|
| `↑` `↓` (or `k` `j`) | Select |
| `Enter` | Expand: full prompt + last 5 actions |
| `Esc` | Collapse, then deselect |
| `o` / double-click | Open the session |
| `n` / click the "Your office" sign | Open the next one (the one in the visitor chair) |
| `w` / click the panel title | Open the Claude desktop app's list of sessions waiting for input |
| `p` | Dispatch a role (see [Roles](#roles)) |
| `r` | Summarize again |
| `b` | Toggle the labels over heads |
| `[` `]` (or `PageUp` `PageDown`) | Change floor |
| `t` | Toggle always-on-top |
| `l` | Switch between English and Chinese, saved to `~/.config/agent-office/env` |
| `q` twice | Quit |

Click a card or worker to select it; scroll the panel with the mouse wheel. After a notification, coming back to the window selects the first session waiting for you, so `o` goes straight to it.

`agent-office --shot out.png` runs for 20 seconds, saves the frame as a PNG and exits.

### Roles

`agent-office roles install` installs nine roles as Claude Code agents (`~/.claude/agents/office-*.md`): product, UX, architect, frontend, backend, QA, devops, reviewer and a PM that splits work among the others. A session started in a role wears its chip on its badge and card; a PM's todo list shows as its plan on its card and on the meeting room's whiteboard.

Press `p` to dispatch one. A digit picks the role and the Claude desktop app opens a new session with the prompt filled in, for you to send:

- On a finished or stuck card, it is a relay: the posts to hand on to come first, marked, and the task carries what the session was asked, what it came to and where its changes are.
- Anywhere else it opens a fresh session in that worker's repository (with none selected, the latest one's) and asks you for its task; until you give one it waits in the waiting area.

From a terminal: `agent-office dispatch --role qa "run the full test suite"` (`--dir`, `--terminal`, `--auto` for the PM). `agent-office roles remove` takes them out again.

## Configuration

All settings are environment variables. The three summary variables and `AGENT_OFFICE_WEATHER` are also read from `~/.config/agent-office/env` (one `KEY=value` per line) when they aren't set.

| Variable | Effect |
|---|---|
| `AGENT_OFFICE_API_BASE` / `_API_KEY` / `_MODEL` | Summary endpoint. Summaries are on only when all three are set |
| `AGENT_OFFICE_SUMMARY=off` | Send nothing out; the panel shows raw prompts |
| `AGENT_OFFICE_LANG` | UI and summary language: `en` or `zh`. Default: the macOS UI language, else `LANG`; anything but Chinese gets English |
| `AGENT_OFFICE_NOTIFY=off` | No system notifications or Dock bounce; the badge still shows |
| `AGENT_OFFICE_SOUND=on` | Sound effects (off by default): a ding when a session starts waiting for approval, a pop with confetti |
| `AGENT_OFFICE_STUCK_FAILS` / `_STUCK_QUIET` / `_STUCK_QUIET_LONG` | Stuck thresholds: failures in a row (default 3), quiet minutes while working (default 5), quiet minutes during long calls (default 20). `doctor` shows the current values |
| `AGENT_OFFICE_THEME` | Theme: `harbour` (default) or `paper`, plus pixtuoid's `normal`, `cyberpunk`, `dracula`, `tokyo-night`, `catppuccin` and `gruvbox` |
| `AGENT_OFFICE_UTC_OFFSET` | Time zone for the sky, lunch, overtime and holidays, in seconds (default `28800`, Hong Kong) |
| `AGENT_OFFICE_WEATHER=hko` | Weather and typhoon signals from the Hong Kong Observatory's open data (one request every 10 minutes; sends nothing of yours) |
| `AGENT_OFFICE_SOCKET` | Hook socket path (default `/tmp/agent-office-<uid>/agent-office.sock`) |
| `AGENT_OFFICE_DEBUG=<file>` | Append every summary result to this file |

## Privacy

With summaries on, each session's prompts, todos, recent actions and the last reply of each turn are sent to **the endpoint you configured**. With no endpoint, or with `AGENT_OFFICE_SUMMARY=off`, no session content leaves your machine. There is no default endpoint in the code.

Daily stats are stored locally in `~/.config/agent-office/stats.json`.

## Limitations

- Only used on macOS.
- Jumping: sessions in the Claude desktop app open directly. For sessions in a terminal, the terminal comes to the front; iTerm2 and Terminal.app also switch to the right tab (macOS asks once for permission to control the terminal). Other terminals can't be taken to a specific tab.
- Sessions in tmux switch to their pane, but only on tmux's default socket (sessions started with `-L` / `-S` can't be found).
- Notifications go through `osascript`, so they appear as "Script Editor" in Notification Center and clicking one doesn't jump to the session; use the Dock icon or the window.
- It can run alongside pixtuoid; each uses its own socket and hooks.

<details>
<summary><b>Little details in the office</b></summary>

- **Harbour view**: the water and reflections, the skyline across the harbour with the ICC, Lion Rock, the Star Ferry, a red-sailed junk, jetfoils, black kites, blinking planes at night, and the Symphony of Lights at 8 pm. The sky follows Hong Kong time.
- **A Central office**: a Hang Seng ticker, a Guan Yu shrine, an arowana tank in the lift lobby, a cha chaan teng corner, money plants and a waving lucky cat, and a shop cat that wanders around.
- **Workers**: workers from the same repo always look the same; Codex workers wear caps, Cursor workers wear headphones, dsh workers wear glasses.
- **Celebrations**: tests going green, `git push`, opening a PR, or finally passing after failures throws confetti, and coworkers clap.
- **Overtime**: after 30 minutes in one turn, or past 10 pm, a cup noodle and milk tea appear on the desk; at night only the overtime desks stay lit.
- **Stuck**: sweat for repeated failures, "…" for silence, a rain cloud when it lasts too long; the shop cat goes to sit at the feet of the worst-off worker.
- **Cantonese chatter**: meeting and pantry small talk is in Cantonese; on weekdays 12:30–13:30 idle workers head down to the cha chaan teng. Click an idle worker and it stretches or talks back.
- **Today's stats**: after 6 pm a neon board shows turns finished, stuck counts and tokens; click it for a 10-second replay of the day and an award ceremony.
- **Holidays**: Lunar New Year, Mid-Autumn and Christmas decorations.
- **Typhoons**: with `AGENT_OFFICE_WEATHER=hko`, signal No. 8 or above stops the ferry, closes the rooftop, and sends everyone packing.

</details>

## Development

```bash
cargo test --release
```

`vendor/` holds modified copies of pixtuoid's `pixtuoid-core` / `pixtuoid-scene` 0.21.0 and of softbuffer, wired in through `[patch.crates-io]` in `Cargo.toml`. Every change is marked `agent-office`. Read the pixtuoid crate's `AGENTS.md` before changing it.

The art scripts are in `tools/`: `people.py` draws the chibi workers and hairstyles, `hk_art.py` the cha chaan teng, plants and furniture colours. They read the upstream sprites from the cargo registry (if they aren't there, comment out `[patch.crates-io]` and run `cargo fetch` once), so they can be re-run. After changing art, run `SNAPSHOTS=overwrite cargo test --release` in `vendor/pixtuoid-scene` to update the pixel snapshots.

## Credits and license

agent-office is released under the MIT License; see [LICENSE](LICENSE).

- The office renderer, animation and data sources come from [pixtuoid](https://github.com/pixtuoid/pixtuoid) (MIT, Copyright (c) 2026 Ivan Wang). `src/pipeline.rs` is adapted from pixtuoid's `runtime/`, and the scaling and name-tag layout in `src/floating/mod.rs` follow pixtuoid.
- `vendor/softbuffer` comes from [softbuffer](https://github.com/rust-windowing/softbuffer) (MIT or Apache-2.0).
- The Fusion Pixel 8px font ships with pixtuoid-scene under the SIL OFL 1.1; see `assets/OFL-Fusion-Pixel.txt` and `assets/LICENSES-Fusion-Pixel/`.
