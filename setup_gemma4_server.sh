#!/bin/bash
set -e

# Configuration
USER_HOME="/home/$(whoami)"
MODELS_DIR="$USER_HOME/models"
SCRIPTS_DIR="$USER_HOME/Scripts"
SRC_DIR="$USER_HOME/llama-cpp-turboquant"
PORT=12345

# Model: Gemma 4 26B A4B Instruction-Tuned Q5_K_S (Unsloth UD quant)
GGUF_URL="https://huggingface.co/unsloth/gemma-4-26B-A4B-it-GGUF/resolve/main/gemma-4-26B-A4B-it-UD-Q5_K_S.gguf"
GGUF_FILE="gemma-4-26B-A4B-it-UD-Q5_K_S.gguf"

# Chat template — bundled alongside this script
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
JINJA_SRC="$SCRIPT_DIR/chat_template.jinja"


echo "[1/7] Installing dependencies..."
sudo apt update
sudo apt install -y build-essential cmake git jq wget curl

echo "[2/7] Cloning and building TurboQuant llama.cpp fork..."
if [ ! -d "$SRC_DIR" ]; then
    git clone https://github.com/TheTom/llama-cpp-turboquant "$SRC_DIR"
fi
cd "$SRC_DIR"
git checkout feature/turboquant-kv-cache
mkdir -p build && cd build
export PATH=$PATH:/usr/local/cuda/bin
cmake .. -DGGML_CUDA=ON
cmake --build . --config Release -j $(nproc) --target llama-server

echo "[3/7] Downloading model and installing chat template..."
mkdir -p "$MODELS_DIR"
wget -c -O "$MODELS_DIR/$GGUF_FILE" "$GGUF_URL"
cp "$JINJA_SRC" "$MODELS_DIR/chat_template.jinja"
echo "Installed chat_template.jinja -> $MODELS_DIR/chat_template.jinja"

echo "[4/7] Creating runner script..."
cat << R_EOF > "$USER_HOME/run_llama_gemma4.sh"
#!/bin/bash
export PATH=\$PATH:/usr/local/cuda/bin
$SRC_DIR/build/bin/llama-server \
  --host 0.0.0.0 \
  -m "$MODELS_DIR/$GGUF_FILE" \
  --cache-type-k turbo3 \
  --cache-type-v turbo3 \
  --flash-attn on \
  --ctx-size 262144 \
  --gpu-layers 99 \
  --port $PORT \
  --alias "Gemma-4-26B-TurboQuant-262k" \
  --reasoning on \
  --jinja \
  --chat-template-file "$MODELS_DIR/chat_template.jinja" \
  --temp 0.1 \
  --repeat-penalty 1.09 \
  --sleep-idle-seconds 30s
R_EOF
chmod +x "$USER_HOME/run_llama_gemma4.sh"

echo "[5/7] Setting up systemd service..."
sudo bash -c "cat << S_EOF > /etc/systemd/system/gemma4.service
[Unit]
Description=Llama Server Gemma 4 26B TurboQuant
After=network.target nvidia-persistenced.service

[Service]
Type=simple
User=$(whoami)
WorkingDirectory=$USER_HOME
ExecStart=$USER_HOME/run_llama_gemma4.sh
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
S_EOF"
sudo systemctl daemon-reload
sudo systemctl enable gemma4.service

echo "[6/7] Creating management scripts in $SCRIPTS_DIR..."
mkdir -p "$SCRIPTS_DIR"

cat << G_EOF > "$SCRIPTS_DIR/start_gemma4.sh"
#!/bin/bash
echo "Stopping Ollama and starting Gemma 4 ($PORT)..."
sudo systemctl stop ollama 2>/dev/null || true
sudo systemctl start gemma4
G_EOF

cat << O_EOF > "$SCRIPTS_DIR/start_ollama.sh"
#!/bin/bash
echo "Stopping Gemma 4 and starting Ollama..."
sudo systemctl stop gemma4
sudo systemctl start ollama
O_EOF

cat << ST_EOF > "$SCRIPTS_DIR/status_ai.sh"
#!/bin/bash
echo '--- AI Service Status ---'
systemctl is-active ollama && echo 'Ollama: RUNNING' || echo 'Ollama: stopped'
systemctl is-active gemma4 && echo "Gemma 4 ($PORT): RUNNING" || echo "Gemma 4 ($PORT): stopped"
ST_EOF

chmod +x "$SCRIPTS_DIR"/*.sh

echo "[7/7] Starting service..."
sudo systemctl start gemma4
echo ""
echo "Setup complete. gemma4.service is active on port $PORT."
echo "Test: curl http://localhost:$PORT/v1/chat/completions -d '{\"model\":\"Gemma-4-26B-TurboQuant-262k\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}],\"max_tokens\":20}' -H 'Content-Type: application/json'"
