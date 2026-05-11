#!/bin/bash
set -e

# Qwen3 27B server — runs on port 7211, hot-swaps GPU with Gemma4 on port 7210.
# Prerequisites: TurboQuant llama.cpp already built (run setup_gemma4_server.sh first).

USER_HOME="/home/$(whoami)"
MODELS_DIR="$USER_HOME/models"
SRC_DIR="$USER_HOME/llama-cpp-turboquant"
PORT=7211

# Model: Qwen3.6-27B-MTP Q4_K_M (16GB, from RDson/Qwen3.6-27B-MTP-Q4_K_M-GGUF on HuggingFace)
GGUF_FILE="Qwen3.6-27B-MTP-Q4_K_M.gguf"
GGUF_PATH="$MODELS_DIR/$GGUF_FILE"    # default location (beside Gemma4)

# Allow override via first argument
if [ -n "$1" ]; then
    GGUF_PATH="$1"
fi

if [ ! -f "$GGUF_PATH" ]; then
    echo "Model not found at $GGUF_PATH"
    echo "Download with:"
    echo "  pip install huggingface_hub --break-system-packages"
    echo "  python3 -c \"from huggingface_hub import hf_hub_download; hf_hub_download('RDson/Qwen3.6-27B-MTP-Q4_K_M-GGUF', 'Qwen3.6-27B-MTP-Q4_K_M.gguf', local_dir='$USER_HOME/')\""
    echo "Or place any Qwen3-27B GGUF at $GGUF_PATH and re-run."
    exit 1
fi

echo "[1/4] Creating runner script..."
cat << R_EOF > "$USER_HOME/run_llama_qwen3.sh"
#!/bin/bash
export PATH=\$PATH:/usr/local/cuda/bin
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
  --sleep-idle-seconds 30s
R_EOF
chmod +x "$USER_HOME/run_llama_qwen3.sh"
echo "  Created $USER_HOME/run_llama_qwen3.sh"

echo "[2/4] Setting up systemd service..."
sudo bash -c "cat << S_EOF > /etc/systemd/system/qwen3.service
[Unit]
Description=Llama Server Qwen3 27B (TurboQuant, port 7211)
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

  - name: Qwen3 27B
    url: http://localhost:$PORT/v1/responses
    model: Qwen3-27B-MTP-Q4
    parser: qwen3
C_EOF
    echo "  Appended Qwen3 entry to $CONFIG_FILE"
else
    echo "  Skipping config update (already present or config not found)"
fi

echo ""
echo "Setup complete. qwen3.service is active on port $PORT."
echo "Both services use --sleep-idle-seconds 30s — only the active one occupies GPU VRAM."
echo ""
echo "Test: curl http://localhost:$PORT/v1/models"
echo "Status: ~/Scripts/status_ai.sh"
