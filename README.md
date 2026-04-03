# Glorfindel Open-Systems

An agentic AI framework built on OMS (Open Mission Systems) principles with a dual-transport pub/sub architecture.

## Architecture

```
                    +---------------------------+
                    |      CONTROL PLANE        |
                    |     (DDS / dust_dds)      |
                    |                           |
                    |  Topics:                  |
                    |  - tasks/request          |
                    |  - tasks/response         |
                    |  - agents/capability      |
                    |  - system/health          |
                    +---------------------------+
                         |              |
              subscribe  |              | publish
                         v              v
+----------------+  +-----------+  +-----------+
|  ORCHESTRATOR  |  |   AGENT   |  |   AGENT   |
|                |  | (Ollama)  |  |  (custom)  |
|  Router        |  +-----------+  +-----------+
|  TaskManager   |       |              |
+----------------+       | execute      |
                         v              v
                    +---------------------------+
                    |       DATA PLANE          |
                    |    (ZeroMQ / tmq)          |
                    |                           |
                    |  PUSH/PULL: tool calls    |
                    |  PUB/SUB:  tool results   |
                    +---------------------------+
                              |
                              v
                    +---------------------------+
                    |     TOOL EXECUTOR         |
                    |  (permission-gated)       |
                    |                           |
                    |  file.read | file.write   |
                    |  bash.exec | search.grep  |
                    +---------------------------+
```

**Why two transports?** Following OMS standards, the control plane (DDS) provides reliable, discoverable pub/sub for task routing and agent registration. The data plane (ZeroMQ) provides high-throughput, low-latency messaging for tool calls and streaming output. This separation mirrors how real mission systems operate.

## Core Message Types

| Message | Plane | Purpose |
|---------|-------|---------|
| `TaskRequest` | Control (DDS) | Submit work to the system |
| `AgentResponse` | Control (DDS) | Agent's result for a task |
| `CapabilityManifest` | Control (DDS) | Agent self-registration |
| `ToolCall` | Data (ZMQ) | Agent requests tool execution |
| `ToolResult` | Data (ZMQ) | Tool execution result |

All messages are wrapped in a `MessageEnvelope<T>` providing correlation IDs, timestamps, and source tracking.

## Project Structure

```
crates/
  glorfindel-schemas/       # Message type definitions (Serde)
  glorfindel-transport/     # DDS + ZMQ transport abstraction
  glorfindel-tools/         # Tool trait + built-in tools
  glorfindel-agent/         # Agent trait, registry, Ollama impl
  glorfindel-orchestrator/  # Task routing + lifecycle management
```

## Quick Start

### Prerequisites

- Rust 1.75+
- Docker + Docker Compose
- NVIDIA GPU + drivers (for Ollama)

### Run with Docker

```bash
# Clone and deploy — models auto-provision on first boot
git clone https://github.com/YOUR_USERNAME/glorfindel-open.git
cd glorfindel-open

# Set which models to pull (comma-separated)
export GLORFINDEL_MODELS=mistral,codellama

# Launch everything
docker compose up -d
```

The orchestrator will:
1. Wait for Ollama to be healthy
2. Pull any models listed in `GLORFINDEL_MODELS`
3. Register the Ollama agent via DDS
4. Listen for tasks

### Run Locally

```bash
# Start Ollama separately
ollama serve &

# Build and run
cargo build --release
GLORFINDEL_MODELS=mistral ./target/release/glorfindel
```

### Run Examples

```bash
# Submit a task directly to an Ollama agent
cargo run --example simple_task

# Register an agent and display its capability manifest
cargo run --example register_agent
```

## Configuration

| Environment Variable | Default | Description |
|---------------------|---------|-------------|
| `OLLAMA_HOST` | `http://localhost:11434` | Ollama API endpoint |
| `GLORFINDEL_MODELS` | `mistral` | Models to auto-pull on startup |
| `DDS_DOMAIN_ID` | `0` | DDS domain for isolation |
| `ZMQ_TOOL_CALL_ENDPOINT` | `tcp://127.0.0.1:5555` | ZMQ PUSH/PULL for tool calls |
| `ZMQ_TOOL_RESULT_ENDPOINT` | `tcp://127.0.0.1:5556` | ZMQ PUB/SUB for tool results |
| `RUST_LOG` | `info` | Log level filter |

## Extending

### Add a Custom Agent

Implement the `Agent` trait:

```rust
use glorfindel_agent::{Agent, AgentError};
use glorfindel_schemas::{TaskRequest, AgentResponse, CapabilityManifest};

struct MyAgent { /* ... */ }

#[async_trait]
impl Agent for MyAgent {
    fn capability(&self) -> CapabilityManifest {
        // Declare what you can do
    }

    async fn handle_task(&self, task: TaskRequest) -> Result<AgentResponse, AgentError> {
        // Your agentic loop here
    }
}
```

### Add a Custom Tool

Implement the `Tool` trait:

```rust
use glorfindel_tools::{Tool, ToolError};
use glorfindel_schemas::tool::ToolResult;

struct MyTool;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &str { "my.tool" }
    fn description(&self) -> &str { "Does something useful" }
    fn required_permissions(&self) -> Vec<Permission> { vec![Permission::Custom("my_perm".into())] }

    async fn execute(&self, task_id: Uuid, params: Value) -> Result<ToolResult, ToolError> {
        // Your tool logic
    }
}
```

### Swap Transports

Implement `ControlPlane` or `DataPlane` traits for your transport of choice (NATS, Redis Streams, RabbitMQ, etc.).

## Design Principles

- **Message-first**: If it's not a message, it doesn't exist in the system
- **Deny-by-default**: Tools require explicit permission grants per task
- **Swap anything**: Every layer has a trait boundary — replace DDS, ZMQ, Ollama, or any tool
- **Self-provisioning**: Deploy the container, models download automatically
- **OMS-derived**: Architecture follows Open Mission Systems patterns for interoperability

## License

MIT
