# Dotsquares-Ai - NixOS Build & Development Commands

# Show available commands
default:
    @just --list

# Enter the Nix development environment
shell:
    nix-shell

# Install frontend dependencies
install:
    npm install

# Build the React frontend
build-frontend:
    npm run build

# Build the Tauri backend (debug)
build-backend:
    cd src-tauri && cargo build

# Build the Tauri backend (release)
build-backend-release:
    cd src-tauri && cargo build --release

# Build everything (frontend + backend)
build: install build-frontend build-backend

# Run the application in development mode
run: build-frontend
    cd src-tauri && cargo run

# Run the application (release mode)
run-release: build-frontend build-backend-release
    cd src-tauri && cargo run --release

# Clean all build artifacts
clean:
    rm -rf node_modules dist
    cd src-tauri && cargo clean

# Development server (requires frontend build first)
dev: build-frontend
    cd src-tauri && cargo run

# Run tests
test:
    cd src-tauri && cargo test

# Format Rust code
fmt:
    cd src-tauri && cargo fmt

# Check Rust code
check:
    cd src-tauri && cargo check

# Quick development cycle: build frontend and run
quick: build-frontend
    cd src-tauri && cargo run

# Full rebuild from scratch
rebuild: clean build run

# Run web server mode for phone access
web: build-frontend
    cd src-tauri && cargo run --bin dotsquares-ai-web

# Run web server on custom port
web-port PORT: build-frontend
    cd src-tauri && cargo run --bin dotsquares-ai-web -- --port {{PORT}}

# Get local IP for phone access
ip:
    @echo "🌐 Your PC's IP addresses:"
    @ip route get 1.1.1.1 | grep -oP 'src \K\S+' || echo "Could not detect IP"
    @echo ""
    @echo "📱 Use this IP on your phone: http://YOUR_IP:8080"

# Show build information
info:
    @echo "🚀 Dotsquares-Ai - Claude Code GUI Application"
    @echo "Built for NixOS without Docker"
    @echo ""
    @echo "📦 Frontend: React + TypeScript + Vite"
    @echo "🦀 Backend: Rust + Tauri"
    @echo "🏗️  Build System: Nix + Just"
    @echo ""
    @echo "💡 Common commands:"
    @echo "  just run      - Build and run (desktop)"
    @echo "  just web      - Run web server for phone access"
    @echo "  just quick    - Quick build and run"
    @echo "  just rebuild  - Full clean rebuild"
    @echo "  just shell    - Enter Nix environment"
    @echo "  just ip       - Show IP for phone access"