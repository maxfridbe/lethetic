#!/bin/bash
set -e

# Qwen3 27B MTP server — runs on port 7211, hot-swaps GPU with Gemma4 on port 7210.
#
# IMPORTANT: Uses ikawrakow/ik_llama.cpp — NOT the TurboQuant fork.
# Standard llama.cpp strips MTP (Multi-Token Prediction) tensors; ik_llama.cpp preserves them.
# The -mtp flag enables ~20% generation speedup via speculative decoding at zero quality cost.

USER_HOME="/home/$(whoami)"
MODELS_DIR="$USER_HOME/models"
IK_DIR="$USER_HOME/ik_llama.cpp"
PORT=7211

# Model: Qwen3.6-27B-MTP Q4_K_M (16GB) — requires MTP-aware llama.cpp
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
    echo "  python3 -c \\"
    echo "    from huggingface_hub import hf_hub_download;"
    echo "    hf_hub_download('RDson/Qwen3.6-27B-MTP-Q4_K_M-GGUF', 'Qwen3.6-27B-MTP-Q4_K_M.gguf', local_dir='$MODELS_DIR/')"
    exit 1
fi

echo "[1/5] Cloning and building ik_llama.cpp (MTP-aware llama.cpp fork)..."
if [ ! -d "$IK_DIR" ]; then
    git clone https://github.com/ikawrakow/ik_llama.cpp "$IK_DIR"
fi
cd "$IK_DIR"
git pull origin main
mkdir -p build && cd build
export PATH=$PATH:/usr/local/cuda/bin
cmake .. -DGGML_CUDA=ON -DCMAKE_BUILD_TYPE=Release
cmake --build . --config Release -j $(nproc) --target llama-server
echo "  Built: $IK_DIR/build/bin/llama-server"

echo "[2/5] Creating runner script..."
cat << R_EOF > "$USER_HOME/run_llama_qwen3.sh"
#!/bin/bash
export PATH=\$PATH:/usr/local/cuda/bin
$IK_DIR/build/bin/llama-server \\
  --host 0.0.0.0 \\
  -m "$GGUF_PATH" \\
  --flash-attn on \\
  --ctx-size 262144 \\
  --gpu-layers 99 \\
  --port $PORT \\
  --alias "Qwen3-27B-MTP-Q4" \\
  --reasoning on \\
  --jinja \\
  --temp 0.2 \\
  --repeat-penalty 1.05 \\
  --sleep-idle-seconds 30s \\
  -mtp \\
  --draft-max 1 \\
  --draft-p-min 0.0
R_EOF
chmod +x "$USER_HOME/run_llama_qwen3.sh"
echo "  Created $USER_HOME/run_llama_qwen3.sh"

echo "[3/5] Setting up systemd service..."
sudo bash -c "cat << S_EOF > /etc/systemd/system/qwen3.service
[Unit]
Description=Llama Server Qwen3 27B MTP (ik_llama.cpp, port 7211)
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

echo "[4/5] Starting service..."
sudo systemctl start qwen3

echo "[5/5] Adding to lethetic config..."
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
echo "Binary: $IK_DIR/build/bin/llama-server"
echo "Flags: -mtp --draft-max 1 --draft-p-min 0.0 (MTP speculative decoding)"
echo ""
echo "Test:   curl http://localhost:$PORT/v1/models"
echo "Status: ~/Scripts/status_ai.sh"
