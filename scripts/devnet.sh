#!/bin/bash
# 4-node localhost devnet for Week 6 testing

set -e

# Build first
echo "🔨 Building novai-node..."
cargo build --release -p novai-node

BIN="./target/release/novai-node"

# Kill any existing nodes
pkill -f "novai-node run" || true
sleep 1

echo "🚀 Starting 4-node devnet..."
echo ""

# Each node gets a unique RPC port (3030..=3033) so all 4 bind cleanly.
# Without distinct --rpc-port values, nodes 1..3 silently fail to bind the
# default RPC port 3030 and only node 0 is reachable over JSON-RPC.

# Node 0: First node (no peers initially)
echo "Starting node 0 on port 9000 (rpc 3030)..."
$BIN run --port 9000 --rpc-port 3030 --dev-keys --allow-insecure-dev-keys --validator 0 > /tmp/node0.log 2>&1 &
sleep 1

# Node 1: Connects to node 0
echo "Starting node 1 on port 9001 (rpc 3031)..."
$BIN run --port 9001 --rpc-port 3031 --peer 127.0.0.1:9000 --dev-keys --allow-insecure-dev-keys --validator 1 > /tmp/node1.log 2>&1 &
sleep 1

# Node 2: Connects to nodes 0 and 1
echo "Starting node 2 on port 9002 (rpc 3032)..."
$BIN run --port 9002 --rpc-port 3032 --peer 127.0.0.1:9000 --peer 127.0.0.1:9001 --dev-keys --allow-insecure-dev-keys --validator 2 > /tmp/node2.log 2>&1 &
sleep 1

# Node 3: Connects to nodes 0, 1, 2
echo "Starting node 3 on port 9003 (rpc 3033)..."
$BIN run --port 9003 --rpc-port 3033 --peer 127.0.0.1:9000 --peer 127.0.0.1:9001 --peer 127.0.0.1:9002 --dev-keys --allow-insecure-dev-keys --validator 3 > /tmp/node3.log 2>&1 &
sleep 1


echo ""
echo "✅ All 4 nodes started!"
echo ""
echo "📋 Logs:"
echo "   Node 0: tail -f /tmp/node0.log"
echo "   Node 1: tail -f /tmp/node1.log"
echo "   Node 2: tail -f /tmp/node2.log"
echo "   Node 3: tail -f /tmp/node3.log"
echo ""
echo "🔍 To watch all logs: tail -f /tmp/node*.log"
echo "⏹️  To stop: pkill -f 'novai-node run'"
echo ""
echo "Waiting 5 seconds for network to stabilize..."
sleep 5

echo ""
echo "📊 Network status:"
for i in 0 1 2 3; do
    echo "Node $i (libp2p 900$i, rpc 303$i): running"
done
