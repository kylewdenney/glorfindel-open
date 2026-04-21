# Glorfindel

A local-first agentic AI framework built on OMS pub/sub principles — and a TTRPG Dungeon Master that runs entirely on your machine.

> **Just want to play?** See [CAMPAIGNS.md](CAMPAIGNS.md).

## What is this?

Glorfindel is two things:

**1. An agentic AI framework** built in Rust. Agents communicate over a dual-transport bus (ZeroMQ data plane, DDS control plane), execute permission-gated tools, and run inference through a local Ollama instance. No cloud required.

**2. A DM Dashboard** — a full web UI for running tabletop RPG sessions using local LLMs. Ships with two public domain campaigns: *Ravenmoor* (gothic horror, Sessions 1–7) and *Avalon* (Gaelic Fae Arthurian, Session 1 complete + Session 2 opening). Both include full pipeline logs for transparency.

---

## DM Dashboard

```bash
cargo build --bin glorfindel-admin
GLORFINDEL_DATA_DIR=./glorfindel-data ./target/debug/glorfindel-admin
# → http://localhost:3000/campaign
```

Requires [Ollama](https://ollama.com) running locally with `mistral` pulled:

```bash
ollama pull mistral
```

### Features

- **Session pipeline** — Thinker → Critic → Rules Lawyer → Campaign Referencer → Dice Roller → DM Writer. Every step traced to a `.meta/*.log` file.
- **Player Turn pipeline** — What Happened Critic → Rules Assessor → Dice Executor → DM Writer → Summarizer. Pick a character, describe an action, get a scene.
- **Play Mode** — toggle the dashboard into player mode: select your character, type what you do, hit Take Action.
- **Live message bus** — ZMQ PUB/SUB + WebSocket feed. Watch every agent spawn, tool call, and file write in real time on the bus panel.
- **Chat-room log viewer** — open any `.meta/*.log` and see the full control plane as colour-coded chat bubbles. Each pipeline stage is its own message.
- **Map-reduce session summarizer** — condenses each turn individually, then synthesises a 5-paragraph recap. Fits inside a 7B context window.
- **Grand Opener / Eucatastrophe** — reads the previous session's summary, extracts the darkest thread, adds a twist, and writes the opening scene of the next session in media res.
- **Dice roller, lore lookup, rules query** — all wired to the same local agent infrastructure.

### Campaigns — Public Domain Playgrounds

See [CAMPAIGNS.md](CAMPAIGNS.md) for the full player-facing guide.

**Ravenmoor** — gothic horror, `glorfindel-data/campaigns/Ravenmoor/`

| Session | Hook |
|---------|------|
| 1 | The eastern well has run dry |
| 2 | Something is in the mine |
| 3 | The chamber predates the village |
| 4 | Casimir has been awake all night |
| 5 | The water is moving uphill |
| 6 | Father Vane knew all along |
| 7 | The name has faded from the paper |

**Avalon** — Gaelic Fae Arthurian, `glorfindel-data/campaigns/Avalon/`

Roman descendants of the Knights of the Round Table have stumbled onto what actually made Arthur's court work. Six-check system (BLADE, PRESENCE, LORE, RITUAL, OFFERING, VEIL), Devotion mechanics, and Fae ritual rules. Four pre-generated characters included. **Designed for Play Mode** — one player per character, or solo.

All sessions, turn files, and `.meta` control plane logs are included. Use it, fork it, run more sessions, break it. Public domain.

---

## Architecture

```
                    ┌──────────────────────────┐
                    │      CONTROL PLANE       │
                    │      (DDS / dust_dds)    │
                    │  tasks · agents · health │
                    └────────────┬─────────────┘
                                 │
              ┌──────────────────┼──────────────────┐
              ▼                  ▼                  ▼
    ┌──────────────────┐  ┌───────────┐  ┌───────────────┐
    │   ORCHESTRATOR   │  │   AGENT   │  │     AGENT     │
    │  Router          │  │  (Ollama) │  │   (custom)    │
    └──────────────────┘  └─────┬─────┘  └───────┬───────┘
                                │                 │
                    ┌───────────▼─────────────────▼───────┐
                    │           DATA PLANE                │
                    │         (ZeroMQ / tmq)              │
                    │   tool calls · results · bus feed   │
                    └─────────────────┬───────────────────┘
                                      │
                    ┌─────────────────▼───────────────────┐
                    │          TOOL EXECUTOR              │
                    │        (deny-by-default)            │
                    │  file · bash · campaign · rulebook  │
                    │  dice · search · sub-agents         │
                    └─────────────────────────────────────┘
```

The control plane (DDS) handles task routing and agent registration. The data plane (ZMQ) carries tool calls and the live bus feed. The DM Dashboard sits on top of both, publishing every event — agent spawns, tool calls, file writes, pipeline steps — to the WebSocket bus.

## Project Structure

```
crates/
  glorfindel-schemas/       # Message types (Serde)
  glorfindel-transport/     # DDS + ZMQ transport abstraction
  glorfindel-tools/         # Tool trait + built-in tools
  glorfindel-agent/         # Agent trait, registry, Ollama impl
  glorfindel-orchestrator/  # Task routing + lifecycle
  glorfindel-admin/         # DM Dashboard — Axum + Alpine.js

glorfindel-data/
  campaigns/Ravenmoor/      # Public domain gothic horror campaign
  definitions/              # Agent definition JSON files

examples/
  dm_assistant.rs           # Standalone DM agent example
```

## Quick Start

### Prerequisites

- Rust 1.75+
- [Ollama](https://ollama.com) with `mistral` pulled
- Docker + Docker Compose (optional)

### Run the DM Dashboard

```bash
git clone https://github.com/kylewdenney/glorfindel-open.git
cd glorfindel-open

ollama pull mistral

cargo build --bin glorfindel-admin
GLORFINDEL_DATA_DIR=./glorfindel-data ./target/debug/glorfindel-admin
```

Open `http://localhost:3000/campaign`. Select Ravenmoor, pick a session, run a turn.

### Run with Docker

```bash
export GLORFINDEL_MODELS=mistral
docker compose up -d
```

## Configuration

| Variable | Default | Description |
|----------|---------|-------------|
| `GLORFINDEL_DATA_DIR` | `./data` | Campaign data and definitions |
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama API endpoint |
| `GLORFINDEL_MODELS` | `mistral` | Models to pull on startup |
| `RUST_LOG` | `info` | Log level |

## Extending

### Add a Tool

```rust
use glorfindel_tools::{Tool, ToolError};

struct MyTool;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str { "my.tool" }
    fn description(&self) -> &str { "Does something useful" }
    fn required_permissions(&self) -> Vec<Permission> { vec![] }

    async fn execute(&self, _task_id: Uuid, params: Value) -> Result<ToolResult, ToolError> {
        // your logic
    }
}
```

### Add an Agent

```rust
use glorfindel_agent::{Agent, AgentError};

struct MyAgent;

#[async_trait]
impl Agent for MyAgent {
    fn capability(&self) -> CapabilityManifest { /* ... */ }
    async fn handle_task(&self, task: TaskRequest) -> Result<AgentResponse, AgentError> { /* ... */ }
}
```

## Design Principles

- **Local-first** — Ollama, ZMQ, everything runs on your machine
- **Observable** — every event on the bus, every pipeline step in a log file
- **Deny-by-default** — tools require explicit permission grants per task
- **Swap anything** — trait boundaries everywhere; replace the model, the transport, any tool
- **OMS-derived** — control/data plane separation follows Open Mission Systems patterns

## License

MIT — including the Ravenmoor campaign data. Do what you want with it.
