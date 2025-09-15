#!/bin/bash

# Rusty Comms - Container Server Management Script for Podman
# This script manages the containerized IPC benchmark server using Podman

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
COMPOSE_FILE="podman-compose.uds.yml"
SERVICE_NAME="rusty-comms-server"

# Derive container name from mechanism to enforce one container per mechanism
get_container_name() {
    case "${MECHANISM:-uds}" in
        pmq) echo "rusty-comms-pmq-server" ;;
        shm) echo "rusty-comms-shm-server" ;;
        uds|*) echo "rusty-comms-uds-server" ;;
    esac
}

# Function to print status
print_status() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

print_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

print_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

# Function to check if podman-compose is available
check_prerequisites() {
    if ! command -v podman-compose &> /dev/null; then
        print_error "podman-compose not found. Please install it:"
        print_error "  sudo dnf install podman-compose -y"
        exit 1
    fi
    
    if ! command -v podman &> /dev/null; then
        print_error "podman not found. Please install it:"
        print_error "  sudo dnf install podman -y"
        exit 1
    fi
}

# Function to start the server
start_server() {
    print_status "Starting containerized IPC benchmark server..."
    
    # Create necessary directories
    mkdir -p sockets output

    local CN
    CN="$(get_container_name)"

    print_status "Using container name: ${CN}"

    # Build image locally
    podman build -t rusty-comms:latest . || { print_error "Image build failed"; exit 1; }

    # Remove any existing same-name container
    podman rm -f "${CN}" >/dev/null 2>&1 || true

    case "${MECHANISM:-uds}" in
        pmq)
            podman run -d --name "${CN}" \
                --ipc=host \
                --security-opt label=disable \
                --user 0:0 \
                -v "$(pwd)/output:/app/output:z" \
                -e RUST_LOG=debug \
                -e IPC_BENCHMARK_OUTPUT_DIR=/app/output \
                localhost/rusty-comms:latest sh -c '
                    umask 000; mkdir -p ./output; \
                    echo "Starting PMQ server (client mode)..."; \
                    ipc-benchmark -m pmq --mode client --msg-count 1000 --message-size 1024 \
                        --output-file ./output/container_server_results.json --log-file stderr
                '
            ;;
        shm)
            podman run -d --name "${CN}" \
                --ipc=host \
                --security-opt label=disable \
                --user 0:0 \
                -v "$(pwd)/output:/app/output:z" \
                -e RUST_LOG=debug \
                -e IPC_BENCHMARK_OUTPUT_DIR=/app/output \
                localhost/rusty-comms:latest sh -c '
                    umask 000; mkdir -p ./output; \
                    echo "Starting SHM server (client mode)..."; \
                    rm -f /dev/shm/ipc_benchmark_shm_crossenv 2>/dev/null || true; \
                    ipc-benchmark -m shm --mode client --shm-name ipc_benchmark_shm_crossenv \
                        --msg-count 1000 --message-size 1024 --buffer-size 65536 \
                        --output-file ./output/container_server_results.json --log-file stderr
                '
            ;;
        uds|*)
            podman run -d --name "${CN}" \
                --security-opt label=disable \
                -v "$(pwd)/sockets:/app/sockets:z" \
                -v "$(pwd)/output:/app/output:z" \
                -e RUST_LOG=debug \
                -e IPC_BENCHMARK_SOCKET_DIR=/app/sockets \
                -e IPC_BENCHMARK_DEFAULT_SOCKET_PATH=/app/sockets/ipc_benchmark.sock \
                -e IPC_BENCHMARK_OUTPUT_DIR=/app/output \
                localhost/rusty-comms:latest sh -c '
                    umask 000; mkdir -p ./sockets ./output; rm -f ./sockets/ipc_benchmark.sock || true; \
                    echo "Starting UDS server (client mode)..."; \
                    ipc-benchmark -m uds --mode client --ipc-path ./sockets/ipc_benchmark.sock \
                        --msg-count 1000 --message-size 1024 \
                        --output-file ./output/container_server_results.json --log-file stderr
                '
            ;;
    esac

    if [ $? -eq 0 ]; then
        print_success "Server container started"
    else
        print_error "Failed to start server container"
        exit 1
    fi
}

# Function to stop the server
stop_server() {
    print_status "Stopping containerized IPC benchmark server..."
    local CN
    CN="$(get_container_name)"
    podman rm -f "${CN}" >/dev/null 2>&1 || true
    print_success "Stopped ${CN}"
}

# Function to show server logs
show_logs() {
    print_status "Showing server logs..."
    local CN
    CN="$(get_container_name)"
    podman logs "${CN}"
}

# Function to show server status
show_status() {
    print_status "Checking server status..."
    podman ps --format 'table {{.Names}}\t{{.Status}}' | grep -E 'rusty-comms-(uds|pmq|shm)-server' || true
}

# Function to restart the server
restart_server() {
    print_status "Restarting containerized IPC benchmark server..."
    stop_server
    sleep 2
    start_server
}

# Function to build the container image
build_image() {
    print_status "Building container image..."
    podman-compose -f "${COMPOSE_FILE}" build
    print_success "Image built successfully!"
}

# Function to show help
show_help() {
    echo -e "${BLUE}=== Rusty Comms Container Server Management ===${NC}"
    echo
    echo "Usage: $0 {start|stop|restart|status|logs|build|help}"
    echo
    echo "Commands:"
    echo "  start    - Start the containerized server"
    echo "  stop     - Stop the containerized server"
    echo "  restart  - Restart the containerized server"
    echo "  status   - Show server status"
    echo "  logs     - Show server logs"
    echo "  build    - Build the container image"
    echo "  help     - Show this help message"
    echo
    echo "Examples:"
    echo "  $0 start          # Start server"
    echo "  $0 logs           # View logs"
    echo "  $0 stop           # Stop server"
    echo
    echo "After starting the server, run the host client with:"
    echo "  ./run_host_client.sh"
}

# Main script logic
main() {
    check_prerequisites
    
    case "${1:-help}" in
        start)
            start_server
            ;;
        stop)
            stop_server
            ;;
        restart)
            restart_server
            ;;
        status)
            show_status
            ;;
        logs)
            show_logs
            ;;
        build)
            build_image
            ;;
        help)
            show_help
            ;;
        *)
            print_error "Unknown command: $1"
            show_help
            exit 1
            ;;
    esac
}

# Run main function with all arguments
main "$@"

