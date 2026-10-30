#!/usr/bin/env python3
"""Verify native Whisper against public speech; no microphone access or audio upload."""
import hashlib
import os
from pathlib import Path
import subprocess
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
CACHE = ROOT / "native/target/dictation-fixtures"
FIXTURES = (
    ("ggml-tiny.bin", "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin",
     "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21", 77_691_713),
    ("jfk.wav", "https://raw.githubusercontent.com/ggml-org/whisper.cpp/master/samples/jfk.wav",
     "59dfb9a4acb36fe2a2affc14bacbee2920ff435cb13cc314a08c13f66ba7860e", 352_078),
)

def main():
    CACHE.mkdir(parents=True, exist_ok=True)
    for name, url, digest, size in FIXTURES:
        path = CACHE / name
        if path.exists() and path.stat().st_size == size and hashlib.sha256(path.read_bytes()).hexdigest() == digest:
            continue
        temporary = path.with_suffix(".download")
        try:
            with urllib.request.urlopen(url, timeout=120) as response, temporary.open("wb") as output:
                received = 0
                while chunk := response.read(65536):
                    received += len(chunk)
                    if received > size:
                        raise ValueError("Fixture exceeded expected size")
                    output.write(chunk)
            if temporary.stat().st_size != size or hashlib.sha256(temporary.read_bytes()).hexdigest() != digest:
                raise ValueError("Fixture checksum mismatch")
            temporary.replace(path)
        finally:
            temporary.unlink(missing_ok=True)
    env = dict(os.environ, LETHETIC_STT_TEST_MODEL=str(CACHE / "ggml-tiny.bin"), LETHETIC_STT_TEST_WAV=str(CACHE / "jfk.wav"))
    subprocess.run(["cargo", "test", "-p", "lethetic-dictation", "--locked", "real_whisper_transcribes", "--", "--ignored"],
                   cwd=ROOT / "native", env=env, check=True, timeout=300)

if __name__ == "__main__":
    main()
