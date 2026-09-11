# ⚡ dotsquares CodAI

> **Next-Generation AI-Assisted Development Workstation & GUI Toolkit for Claude Code**  
> Engineered for developers who demand speed, autonomy, and visual control over AI-driven software engineering.

---

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](LICENSE)
[![Tauri v2](https://img.shields.io/badge/Tauri-v2.0-24C8D8?logo=tauri&logoColor=white)](https://tauri.app/)
[![React](https://img.shields.io/badge/React-18.3-61DAFB?logo=react&logoColor=black)](https://react.dev/)
[![TypeScript](https://img.shields.io/badge/TypeScript-5.6-3178C6?logo=typescript&logoColor=white)](https://www.typescriptlang.org/)
[![Rust](https://img.shields.io/badge/Rust-2021_Edition-dea584?logo=rust&logoColor=black)](https://www.rust-lang.org/)
[![Vite](https://img.shields.io/badge/Vite-6.0-646CFF?logo=vite&logoColor=white)](https://vitejs.dev/)
[![Tailwind CSS](https://img.shields.io/badge/TailwindCSS-4.1-38B2AC?logo=tailwind-css&logoColor=white)](https://tailwindcss.com/)
[![Author](https://img.shields.io/badge/Author-Prajjwal_Jaiswal-blueviolet)](#-author--credits)

---

## 📖 Overview

**dotsquares CodAI** is a cross-platform desktop workstation and web environment designed to unlock the full potential of **Claude Code** and autonomous software engineering agents. While terminal-based AI tools are powerful, complex real-world workflows require visual diff inspection, granular timeline rollbacks, multi-agent orchestration, token telemetry, and task planning.

dotsquares CodAI bridges this gap by wrapping cutting-edge agent runtimes with a high-performance **Tauri v2 + Rust** desktop backend and a reactive **React 18 + TypeScript** interface. It also includes an embedded **Axum WebSocket web server** mode, allowing you to monitor and control agent sessions from your phone, tablet, or secondary monitor seamlessly.

---

## ✨ Key Features

### 🤖 Autonomous Agent Workstation

- **Multi-Agent Orchestration**: Spin up dedicated coding agents to triage issues, refactor modules, write unit tests, and build features autonomously.
- **GitHub Agent Browser**: Discover, download, and execute specialized agent templates and recipes directly from community repositories.
- **Interactive Clarifications**: Human-in-the-loop dialogs (`AgentQuestionDialog`) allow agents to ask clarifying questions before committing irreversible actions.
- **Execution Output Streamer**: Rich real-time logs with ANSI color parsing, formatted tool calls, and error tracking.

### 💻 Dual Execution Modes

- **Native Desktop App**: Built on Tauri v2 for low-memory overhead, native window styling, transparent macOS vibrancy, and high-speed local IPC.
- **Web Server & Remote Mode (`dotsquares-ai-web`)**: Run the built-in Axum web server (`just web`) to interact with Claude Code through any modern web browser or mobile phone on your local network.

### 🔌 Model Context Protocol (MCP) Hub

- **Server Registry & Manager**: Add, inspect, configure, and monitor MCP servers with ease.
- **Tool Discovery & Testing**: Dynamically inspect exposed MCP tools and test execution against live environments.
- **Import / Export**: Easily backup or share your MCP configuration profiles across teams.

### ⏳ Git-Backed Checkpoints & Timeline Time-Travel

- **Automated Micro-Checkpoints**: Automatically snapshot project states prior to executing agent code edits.
- **Interactive Timeline**: Visually inspect step-by-step diffs with side-by-side highlighting.
- **One-Click Rollbacks**: Revert unintended code modifications instantly with zero loss of previous history.

### 📋 Integrated Task Board (Kanban)

- **Agile Ticket Management**: Organize development goals into _To Do_, _In Progress_, and _Completed_ columns.
- **Agent Task Dispatch**: Directly attach agent sessions to tickets for targeted feature implementation.
- **Ticket Import / Export**: Import issues from external issue trackers or export sprint reports as JSON/CSV.

### 📊 Token & Cost Telemetry Dashboard

- **Real-Time Token Monitoring**: Live breakdown of input tokens, output tokens, and cache hits per session.
- **Cost Estimation**: Accurate usage and financial estimation across Claude 3.5 Sonnet, Claude 3 Opus, and custom endpoints.
- **Visual Analytics**: Interactive usage charts powered by Recharts to identify cost drivers across long agent sessions.

### 🛠️ Developer Tooling & Productivity

- **Slash Command Manager**: Quick trigger menu (`/commit`, `/test`, `/explain`, etc.) with customizable user macros.
- **Pre & Post Hooks Editor**: Configure automated shell hooks that trigger before or after agent tasks.
- **Syntax-Highlighted File Viewer & Editor**: Review code, markdown documentation, and media assets inline.
- **Proxy & Custom Base URL Configuration**: Seamless support for corporate HTTP/HTTPS proxies and custom API routing.

---

## 🏗️ Architecture & Tech Stack

```
┌─────────────────────────────────────────────────────────────┐
│                    dotsquares CodAI                         │
├──────────────────────────────┬──────────────────────────────┤
│      Desktop Interface       │     Web Server Interface     │
│   (Tauri v2 Webview Window)  │   (Axum WS / HTTP Server)    │
├──────────────────────────────┴──────────────────────────────┤
│                       React 18 Frontend                     │
│   TypeScript • Tailwind CSS • Radix UI • Zustand • Recharts │
├─────────────────────────────────────────────────────────────┤
│                          Rust Core                          │
│   Tauri 2 • Tokio Async • Rusqlite (SQLite) • Axum • Reqwest│
├──────────────────────────────┬──────────────────────────────┤
│      Claude Code Engine      │      Local Git Workspace     │
│   Process Manager & Streaming│    Checkpoints & Rollbacks   │
└──────────────────────────────┴──────────────────────────────┘
```

| Layer               | Technologies                                                                                                                                           |
| :------------------ | :----------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Frontend UI**     | [React 18](https://react.dev/), [TypeScript](https://www.typescriptlang.org/), [Vite](https://vitejs.dev/), [Tailwind CSS 4](https://tailwindcss.com/) |
| **UI Components**   | [Radix UI](https://www.radix-ui.com/), [Lucide Icons](https://lucide.dev/), [Framer Motion](https://www.framer.com/motion/)                            |
| **State & Data**    | [Zustand](https://github.com/pmndrs/zustand), [TanStack Virtual](https://tanstack.com/virtual), [Recharts](https://recharts.org/)                      |
| **Desktop Runtime** | [Tauri v2](https://tauri.app/) (Rust 2021)                                                                                                             |
| **Backend Core**    | [Tokio](https://tokio.rs/), [Axum](https://github.com/tokio-rs/axum) (WebSockets & REST), [Rusqlite](https://github.com/rusqlite/rusqlite) (SQLite)    |
| **Integrations**    | [Claude Code CLI](https://docs.anthropic.com/en/docs/agents-and-tools/claude-code/overview), Model Context Protocol (MCP), Git VCS                     |

---

## 📦 Prerequisites

Before running or building **dotsquares CodAI**, ensure your system has the following installed:

1. **Node.js / Bun**:
   - [Node.js](https://nodejs.org/) (v18.0 or later) or [Bun](https://bun.sh/) (recommended for fast builds)
2. **Rust & Cargo**:
   - [Rust Toolchain](https://www.rust-lang.org/tools/install) (`rustup default stable`, 1.78+)
3. **Claude Code CLI**:
   - Install globally via npm:
     ```bash
     npm install -g @anthropic-ai/claude-code
     ```
4. **Platform Dependencies** (Tauri v2 Prerequisites):
   - **macOS**: Xcode Command Line Tools (`xcode-select --install`)
   - **Linux** (Debian/Ubuntu):
     ```bash
     sudo apt-get update && sudo apt-get install -y \
       libwebkit2gtk-4.1-dev \
       build-essential \
       curl \
       wget \
       file \
       libxdo-dev \
       libssl-dev \
       libayatana-appindicator3-dev \
       librsvg2-dev
     ```
   - **Windows**: Microsoft C++ Build Tools & WebView2 Runtime

---

## 🚀 Getting Started

### 1. Clone the Repository

```bash
git clone https://github.com/prajjwal/dotsquares-ai.git
cd dotsquares-ai
```

### 2. Install Dependencies

Using Bun (preferred):

```bash
bun install
```

Or using npm:

```bash
npm install
```

### 3. Launch in Development Mode

Run the desktop application with live-reloading:

```bash
# Using Bun / npm
bun run tauri dev

# Or using Justfile
just run
```

### 4. Running in Web Server Mode (Mobile & Remote Access)

If you want to run dotsquares CodAI as a web server to access it from mobile devices or other computers on your local network:

```bash
# Build frontend and start the Axum web server
just web

# Or specify a custom port
just web-port 8080
```

Once running, access the web dashboard via your browser:

```
http://localhost:8080
```

_(Or your local network IP: `http://<your-ip>:8080` for mobile access)_

---

## 💻 Available Scripts & Commands

This project supports standard package runner scripts as well as a [`justfile`](justfile) for streamlined workflows:

### Package Manager Scripts

| Command               | Action                                                 |
| :-------------------- | :----------------------------------------------------- |
| `bun run dev`         | Starts the Vite development server (frontend only)     |
| `bun run build`       | Compiles TypeScript and builds the production frontend |
| `bun run tauri dev`   | Starts both frontend and Tauri desktop application     |
| `bun run tauri build` | Creates production desktop executables / installers    |
| `bun run build:dmg`   | Builds a standalone `.dmg` installer for macOS         |
| `bun run check`       | Runs TypeScript typechecks and Cargo check             |

### Justfile Commands

| Command                      | Action                                                     |
| :--------------------------- | :--------------------------------------------------------- |
| `just run`                   | Builds frontend and starts the Tauri desktop application   |
| `just web`                   | Builds frontend and runs the web server mode               |
| `just web-port <PORT>`       | Starts the web server on a custom port                     |
| `just build-backend`         | Compiles debug Rust backend in `src-tauri`                 |
| `just build-backend-release` | Compiles optimized release Rust backend                    |
| `just test`                  | Executes Rust backend test suite                           |
| `just check`                 | Runs `cargo check` for syntax and type errors              |
| `just fmt`                   | Formats all Rust backend code with `cargo fmt`             |
| `just ip`                    | Displays your machine's local IP for mobile access         |
| `just clean`                 | Removes `dist`, `node_modules`, and Cargo target artifacts |

---

## 📁 Project Directory Structure

```plaintext
dotsquares-ai/
├── cc_agents/             # Agent templates, configurations, and specialized recipes
├── dist/                  # Production frontend build output
├── scripts/               # Binary download, build, and packaging helper scripts
├── src/                   # React 18 Frontend Application
│   ├── assets/            # Static assets and icons
│   ├── components/        # Reusable UI components
│   │   ├── claude-code-session/ # Interactive Claude session panels
│   │   ├── ui/            # Radix UI + Tailwind styled primitives
│   │   ├── widgets/       # Diff viewers, file editors, previews
│   │   ├── AgentExecution.tsx   # Agent lifecycle management
│   │   ├── MCPManager.tsx       # Model Context Protocol control center
│   │   ├── TaskBoard.tsx        # Kanban agile board
│   │   ├── TimelineNavigator.tsx# Checkpoint time-travel viewer
│   │   └── UsageDashboard.tsx   # Token and cost analytics
│   ├── contexts/          # React Context providers
│   ├── hooks/             # Custom React hooks
│   ├── lib/               # Utility functions and API adapters
│   ├── stores/            # Zustand global state stores
│   ├── types/             # TypeScript definitions
│   ├── App.tsx            # Main application root
│   └── styles.css         # Global Tailwind & aesthetic styles
├── src-tauri/             # Rust Backend & Tauri Runtime
│   ├── src/
│   │   ├── checkpoint/    # Git micro-checkpoint and snapshot management
│   │   ├── commands/      # Tauri IPC command handlers (agents, git, mcp, tickets)
│   │   ├── process/       # Process spawning and execution pipelines
│   │   ├── claude_binary.rs # Binary locator and version detection
│   │   ├── main.rs        # Tauri desktop entrypoint
│   │   ├── web_main.rs    # Standalone web server entrypoint
│   │   └── web_server.rs  # Axum HTTP & WebSocket streaming implementation
│   ├── Cargo.toml         # Rust crate dependencies and build configuration
│   └── tauri.conf.json    # Tauri application metadata and security policies
├── templates/             # Project and prompt scaffolding templates
├── justfile               # Just command definitions
├── package.json           # Node/Bun dependencies and scripts
└── README.md              # Project documentation
```

---

## ⚙️ Configuration & Customization

- **Claude Binary Path**: By default, dotsquares CodAI discovers the global `claude` binary on your system `$PATH`. You can override or switch versions in **Settings > Claude Version Selector**.
- **MCP Servers**: Configure your servers in **Settings > MCP Manager** or by importing your existing `claude_desktop_config.json`.
- **Custom Proxies**: Set up corporate HTTP, HTTPS, or SOCKS5 proxies directly inside **Settings > Proxy Settings**.
- **Storage**: Session records, task tickets, and usage stats are persisted locally via embedded SQLite in your system app data folder.

---

## 🤝 Contributing

Contributions to **dotsquares CodAI** are welcome! Please follow these guidelines:

1. **Fork** the repository and create a feature branch (`git checkout -b feature/amazing-feature`).
2. Adhere to the established coding standards:
   - TypeScript + functional React components with Tailwind CSS for frontend code.
   - Standard idiomatic Rust with `cargo fmt` and explicit error handling for backend code.
3. Verify your changes pass checks:
   ```bash
   bun run check
   ```
4. Commit your changes following standard prefix conventions (`Feature:`, `Fix:`, `Docs:`, `Refactor:`).
5. Open a Pull Request detailing your changes, motivations, and testing steps.

For more details, see [CONTRIBUTING.md](CONTRIBUTING.md).

---

## 👤 Author & Credits

**dotsquares CodAI** is created and maintained by:

- **Author**: **Prajjwal Jaiswal**
- **Project**: **dotsquares CodAI**

---

## 📄 License

This project is licensed under the terms of the **GNU Affero General Public License v3.0 (AGPL-3.0)**.  
See the [LICENSE](LICENSE) file for full license terms.
