#!/usr/bin/env sh
# Downloads the English-only quantized Whisper model (~60 MB) used by Vox.
set -eu
dir="$(cd "$(dirname "$0")/.." && pwd)/models"
mkdir -p "$dir"
curl -L --fail -o "$dir/ggml-base.en-q5_1.bin" \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en-q5_1.bin
