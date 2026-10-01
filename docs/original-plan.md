# EndlessVibe Development Plan

## Overview

EndlessVibe is a Rust-based MCP (Model Context Protocol) server.

The goal is to provide a self-hosted AI development agent backend that
connects ChatGPT Web with local machines through MCP.

EndlessVibe allows ChatGPT Agent to:

-   inspect local projects
-   understand workspace structure
-   search code
-   modify files
-   execute development commands
-   manage git workflow
-   continue long-running development tasks

------------------------------------------------------------------------

# Architecture

    ChatGPT Web
         |
         | MCP Protocol
         |
    Cloudflare Tunnel
         |
         |
    localhost:20000
         |
         |
    EndlessVibe MCP Server
         |
         +----------------+
         |
     Workspace Manager
         |
     Tool Runtime
         |
     Agent Engine
         |
     Git Manager
         |
     Task Scheduler

------------------------------------------------------------------------

# Project Structure

    EndlessVibe
    |
    ├── Cargo.toml
    ├── README.md
    |
    ├── src
    │
    │── main.rs
    │
    ├── server
    │   ├── mod.rs
    │   └── mcp.rs
    │
    ├── tools
    │   ├── mod.rs
    │   ├── filesystem.rs
    │   ├── shell.rs
    │   ├── git.rs
    │   ├── project.rs
    │   └── search.rs
    │
    ├── workspace
    │   ├── mod.rs
    │   └── manager.rs
    │
    ├── agent
    │   ├── mod.rs
    │   ├── planner.rs
    │   ├── executor.rs
    │   └── history.rs
    │
    ├── storage
    │   ├── mod.rs
    │   └── sqlite.rs
    │
    └── config
        └── config.toml

------------------------------------------------------------------------

# Phase 0 - Project Initialization

## Goal

Create Rust MCP server foundation.

Tasks:

-   Create Rust project
-   Setup MCP dependencies
-   Prepare module structure

Dependencies:

    tokio
    axum
    serde
    serde_json
    tracing
    anyhow
    uuid
    rmcp

Acceptance:

``` bash
cargo build
```

------------------------------------------------------------------------

# Phase 1 - MCP Server Core

## Goal

Implement MCP HTTP service.

Default endpoint:

    http://127.0.0.1:20000/mcp

Support:

    initialize

    tools/list

    tools/call

Acceptance:

``` bash
curl http://127.0.0.1:20000/mcp
```

------------------------------------------------------------------------

# Phase 2 - Basic Development Tools

## Filesystem

Tools:

    read_file
    write_file
    list_directory

## Code Search

Tool:

    search_code

Implementation:

    ripgrep

------------------------------------------------------------------------

# Phase 3 - Workspace Management

Config:

    ~/.config/endlessvibe/workspace.toml

Example:

``` toml
[[workspace]]
name="BAfter"
path="/home/vv/project/BAfter"

[[workspace]]
name="mHypr"
path="/home/vv/project/mHypr"

[[workspace]]
name="mountain_and_sea"
path="/home/vv/project/mountain_and_sea"
```

Tools:

    list_projects
    inspect_project
    select_workspace

------------------------------------------------------------------------

# Phase 4 - Command Execution

Tool:

    run_command

Security:

Allowed:

    cargo
    cmake
    ninja
    make
    git
    rg
    ls
    find

Blocked:

    rm
    mkfs
    dd
    shutdown

------------------------------------------------------------------------

# Phase 5 - Git Agent

Tools:

    git_status
    git_diff
    git_log
    git_commit

Workflow:

    inspect
    modify
    build
    test
    commit

------------------------------------------------------------------------

# Phase 6 - Agent Engine

Architecture:

    Task
     |
    Planner
     |
    Executor
     |
    Validator
     |
    Reporter

Modules:

    agent/

    planner.rs
    executor.rs
    validator.rs
    history.rs

------------------------------------------------------------------------

# Phase 7 - Persistent Task System

Storage:

SQLite

Tables:

    tasks
    executions
    commands
    changes
    commits

Features:

-   task queue
-   retry
-   execution history
-   failure recovery

------------------------------------------------------------------------

# Phase 8 - Cloudflare Tunnel

Local service:

    127.0.0.1:20000

Start:

``` bash
cargo run
```

Tunnel:

``` bash
cloudflared tunnel \
--url http://localhost:20000
```

Public:

    https://mcp.example.com/mcp

------------------------------------------------------------------------

# Phase 9 - ChatGPT MCP Integration

MCP URL:

    https://mcp.example.com/mcp

Authentication:

Initial:

    none

Future:

    Bearer Token

------------------------------------------------------------------------

# Phase 10 - Intelligence Layer

Support:

    Rust
    C++
    Qt
    Python
    Bevy

Features:

-   build detection
-   dependency analysis
-   project summary

Code intelligence:

    rust-analyzer
    clangd
    tree-sitter

------------------------------------------------------------------------

# Phase 11 - Endless Agent Mode

Example:

User:

    Continue improving /home/vv/project/mHypr

Agent:

    1. Load project state
    2. Read plan
    3. Find next task
    4. Modify code
    5. Validate
    6. Commit
    7. Continue

------------------------------------------------------------------------

# Milestones

## M0

Rust MCP server running.

## M1

ChatGPT can call:

    list_projects
    read_file
    search_code

## M2

Agent can:

    modify
    build
    test
    commit

## M3

EndlessVibe becomes a self-hosted CodexPro alternative.

------------------------------------------------------------------------

# Development Rules

1.  Rust first.
2.  Small independent commits.
3.  Never overwrite user changes.
4.  Always check git status.
5.  Keep execution history.
6.  Prefer incremental validation.
7.  All Agent actions must be auditable.
