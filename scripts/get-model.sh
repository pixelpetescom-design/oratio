#!/usr/bin/env sh
# Downloads a Whisper model for Vox into ./models.
#   get-model.sh base    English-only, ~60 MB, for the CPU build (default)
#   get-model.sh large   large-v3-turbo, ~570 MB, for the GPU build
set -eu
case "${1:-base}" in
  large) file=ggml-large-v3-turbo-q5_0.bin ;;
  *) file=ggml-base.en-q5_1.bin ;;
esac
dir="$(cd "$(dirname "$0")/.." && pwd)/models"
mkdir -p "$dir"
curl -L --fail -o "$dir/$file" "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$file"
