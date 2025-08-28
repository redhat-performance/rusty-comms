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
CONTAINER_NAME="rusty-comms-uds-server"

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
    
    # Start the server in background
    podman-compose -f "${COMPOSE_FILE}" up -d "${SERVICE_NAME}"
    
    if [ $? -eq 0 ]; then
        print_success "Server started successfully!"
        print_status "Waiting for server to initialize..."
        sleep 3
        
        # Check if socket was created
        if [ -S "./sockets/ipc_benchmark.sock" ]; then
            print_success "Socket created successfully at ./sockets/ipc_benchmark.sock"
        else
            print_warning "Socket not yet available. Server may still be starting..."
            print_status "Check logs with: $0 logs"
        fi
    else
        print_error "Failed to start server"
        exit 1
    fi
}

# Function to stop the server
stop_server() {
    print_status "Stopping containerized IPC benchmark server..."
    podman-compose -f "${COMPOSE_FILE}" down
    
    # Clean up socket file
    if [ -f "./sockets/ipc_benchmark.sock" ]; then
        rm -f "./sockets/ipc_benchmark.sock"
        print_status "Cleaned up socket file"
    fi
    
    print_success "Server stopped successfully!"
}

# Function to show server logs
show_logs() {
    print_status "Showing server logs..."
    podman logs "${CONTAINER_NAME}"
}

# Function to show server status
show_status() {
    print_status "Checking server status..."
    
    # Check if container is running
    if podman ps --filter "name=${CONTAINER_NAME}" --format "{{.Names}}" | grep -q "${CONTAINER_NAME}"; then
        print_success "Server container is running"
        
        # Check if socket exists
        if [ -S "./sockets/ipc_benchmark.sock" ]; then
            print_success "Socket is available at ./sockets/ipc_benchmark.sock"
        else
            print_warning "Socket not available yet"
        fi
    else
        print_warning "Server container is not running"
        
        # Check if container exists but is stopped
        if podman ps -a --filter "name=${CONTAINER_NAME}" --format "{{.Names}}" | grep -q "${CONTAINER_NAME}"; then
            print_status "Container exists but is stopped. Last status:"
            podman ps -a --filter "name=${CONTAINER_NAME}" --format "table {{.Names}}\t{{.Status}}\t{{.CreatedAt}}"
        else
            print_status "No container found"
        fi
    fi
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

