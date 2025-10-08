#!/bin/bash

echo "=== CPU Affinity Test Monitor ==="
echo "This script will monitor CPU affinity during benchmark execution"
echo ""

# Check if benchmark is running
if pgrep -f "ipc-benchmark" > /dev/null; then
    echo "⚠️  IPC benchmark is already running. Please stop it first."
    echo "   Use: pkill -f ipc-benchmark"
    exit 1
fi

echo "Instructions:"
echo "1. Run this script in Terminal 1"
echo "2. In Terminal 2, run:"
echo "   cd /home/mcurrier/auto/work/rusty-comms"
echo "   ./target/release/ipc-benchmark -m shm --duration 30s -s 1024 --server-affinity 3 --client-affinity 2"
echo ""
echo "Expected Results:"
echo "  - Server process should stay on CPU 3"
echo "  - Client thread should stay on CPU 2"
echo "  - Main process can move between CPUs"
echo ""
echo "Press Enter when ready to start monitoring..."
read

echo "🔍 Monitoring CPU affinity (Press Ctrl+C to stop)..."
echo ""

while true; do
    # Clear screen for clean output
    clear
    
    echo "=== CPU Affinity Monitor - $(date) ==="
    echo ""
    
    # Check if benchmark is running
    if ! pgrep -f "ipc-benchmark" > /dev/null; then
        echo "⏳ Waiting for ipc-benchmark to start..."
        echo "   Run the benchmark in Terminal 2"
        sleep 2
        continue
    fi
    
    echo "📊 Current CPU Usage:"
    echo "Format: PID   LWP   CPU  COMMAND"
    echo "        │     │     │    └─ Process name"
    echo "        │     │     └─ CPU core number"
    echo "        │     └─ Thread ID (LWP = Light Weight Process)"
    echo "        └─ Process ID"
    echo ""
    
    # Show detailed thread information
    ps -eLo pid,lwp,psr,comm | grep ipc-benchmark | while read line; do
        pid=$(echo $line | awk '{print $1}')
        lwp=$(echo $line | awk '{print $2}')
        psr=$(echo $line | awk '{print $3}')
        comm=$(echo $line | awk '{print $4}')
        
        if [ "$pid" = "$lwp" ]; then
            # This is the main thread of a process
            if [ $(ps -eLo pid,lwp | grep "^$pid " | wc -l) -gt 1 ]; then
                echo "$line  # Main process (multi-threaded)"
            else
                echo "$line  # Server process"
            fi
        else
            # This is a worker thread
            echo "$line  # Client thread"
        fi
    done
    
    echo ""
    echo "🎯 Affinity Check:"
    
    # Count processes on each CPU
    server_cpu=$(ps -eLo pid,lwp,psr,comm | grep ipc-benchmark | awk '$1==$2 && NF==4 {print $3}' | grep -v "$(ps -eLo pid,lwp,psr,comm | grep ipc-benchmark | awk '$1!=$2 {print $1}' | head -1 | xargs -I {} ps -eLo pid,lwp,psr,comm | awk -v pid={} '$1==pid && $1==$2 {print $3}')" 2>/dev/null | head -1)
    client_cpu=$(ps -eLo pid,lwp,psr,comm | grep ipc-benchmark | awk '$1!=$2 {print $3}' | head -1)
    
    if [ ! -z "$server_cpu" ] && [ ! -z "$client_cpu" ]; then
        if [ "$server_cpu" = "3" ]; then
            echo "✅ Server on CPU $server_cpu (Expected: 3)"
        else
            echo "❌ Server on CPU $server_cpu (Expected: 3)"
        fi
        
        if [ "$client_cpu" = "2" ]; then
            echo "✅ Client on CPU $client_cpu (Expected: 2)"
        else
            echo "❌ Client on CPU $client_cpu (Expected: 2)"
        fi
        
        if [ "$server_cpu" = "$client_cpu" ]; then
            echo "⚠️  WARNING: Server and client on same CPU!"
        else
            echo "✅ Server and client on different CPUs"
        fi
    fi
    
    echo ""
    echo "Press Ctrl+C to stop monitoring"
    sleep 2
done


