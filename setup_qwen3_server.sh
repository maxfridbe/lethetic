#!/bin/bash
set -e

# Qwen3 27B MTP server — runs on port 7211.
#
# Uses ik_llama.cpp fork (feature/turboquant-kv at github.com/maxfridbe/ik_llama_tq.cpp)
# which combines:
#   - MTP speculative decode (-mtp, ~20% speedup via built-in multi-token prediction)
#   - turbo3 KV cache (--cache-type-k/v turbo3, ~8x compression → 262k ctx in 24GB)
#   - --sleep-idle-seconds: process stays alive but frees ~20GB VRAM after N idle seconds;
#     reloads from OS page cache (~9s) on next request — same behavior as Gemma4 server.
#
# Build the binary first:
#   cd ~/ik_llama.cpp  # cloned from github.com/maxfridbe/ik_llama_tq.cpp
#   git checkout feature/turboquant-kv
#   mkdir build && cd build
#   cmake .. -DGGML_CUDA=ON -DGGML_CUDA_FA_ALL_QUANTS=ON -DGGML_CUDA_USE_GRAPHS=ON
#   cmake --build . --config Release -j$(nproc) --target llama-server
#
# systemd service: Restart=on-failure (not always — server stays alive through sleep/wake)

USER_HOME="/home/$(whoami)"
MODELS_DIR="$USER_HOME/models"
SRC_DIR="$USER_HOME/ik_llama.cpp"
PORT=7211

# Model: Qwen3.6-27B-MTP Q4_K_M (16GB)
GGUF_FILE="Qwen3.6-27B-MTP-Q4_K_M.gguf"
GGUF_PATH="$MODELS_DIR/$GGUF_FILE"

# Allow override via first argument
if [ -n "$1" ]; then
    GGUF_PATH="$1"
fi

if [ ! -f "$GGUF_PATH" ]; then
    echo "Model not found at $GGUF_PATH"
    echo "Download with:"
    echo "  pip install huggingface_hub --break-system-packages"
    echo "  python3 -c \"from huggingface_hub import hf_hub_download; hf_hub_download('RDson/Qwen3.6-27B-MTP-Q4_K_M-GGUF', 'Qwen3.6-27B-MTP-Q4_K_M.gguf', local_dir='$MODELS_DIR/')\""
    exit 1
fi

# Ensure TurboQuant is built at b9082+ (adds Qwen35/SSM architecture support)
if [ ! -f "$SRC_DIR/build/bin/llama-server" ]; then
    echo "TurboQuant binary not found. Run setup_gemma4_server.sh first."
    exit 1
fi
TQVER=$("$SRC_DIR/build/bin/llama-server" --version 2>&1 | grep 'version:' | awk '{print $2}')
echo "TurboQuant version: $TQVER (need b9082+ for Qwen3 MTP support)"

echo "[1/4] Creating runner script..."
cat << R_EOF > "$USER_HOME/run_llama_qwen3.sh"
#!/bin/bash
export PATH=\$PATH:/usr/local/cuda/bin
# Combined MTP (speculative decode, ~20% speedup) + turbo3 KV cache (8x compression).
# Uses ik_llama.cpp fork with turbo3 CPY + flash-attn fixes (feature/turboquant-kv branch).
$SRC_DIR/build/bin/llama-server \\
  --host 0.0.0.0 \\
  -m "$GGUF_PATH" \\
  --cache-type-k turbo3 \\
  --cache-type-v turbo3 \\
  --flash-attn on \\
  --ctx-size 262144 \\
  --gpu-layers 99 \\
  --port $PORT \\
  --alias "Qwen3-27B-MTP-Q4" \\
  --reasoning on \\
  --jinja \\
  --temp 0.2 \\
  --repeat-penalty 1.05 \\
  -mtp \\
  --draft-max 1 \\
  --draft-p-min 0.0 \\
  --sleep-idle-seconds 120
R_EOF
chmod +x "$USER_HOME/run_llama_qwen3.sh"
echo "  Created $USER_HOME/run_llama_qwen3.sh"

echo "[2/4] Setting up systemd service..."
sudo bash -c "cat << S_EOF > /etc/systemd/system/qwen3.service
[Unit]
Description=Llama Server Qwen3 27B MTP (TurboQuant b9082+, port 7211)
After=network.target nvidia-persistenced.service

[Service]
Type=simple
User=$(whoami)
WorkingDirectory=$USER_HOME
ExecStart=$USER_HOME/run_llama_qwen3.sh
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
S_EOF"
sudo systemctl daemon-reload
sudo systemctl enable qwen3.service

echo "[3/4] Starting service..."
sudo systemctl start qwen3

echo "[4/4] Adding to lethetic config..."
CONFIG_FILE="$HOME/.config/lethetic/config.yml"
if [ -f "$CONFIG_FILE" ] && ! grep -q "qwen3" "$CONFIG_FILE"; then
    cat << C_EOF >> "$CONFIG_FILE"

  - name: Qwen3 27B MTP
    url: http://localhost:$PORT/v1/responses
    model: Qwen3-27B-MTP-Q4
    parser: qwen3
C_EOF
    echo "  Appended Qwen3 entry to $CONFIG_FILE"
else
    echo "  Skipping config update (already present or config not found)"
fi

echo ""
echo "Setup complete. qwen3.service running on port $PORT."
echo "Binary: TurboQuant b9082+ with turbo3 KV cache"
echo "Note: MTP tensors are loaded but speculative decoding is not active."
echo "      For -mtp speedup at the cost of VRAM, use ik_llama.cpp instead."
echo ""
echo "Test:   curl http://localhost:$PORT/v1/models"
echo "Status: ~/Scripts/status_ai.sh"
