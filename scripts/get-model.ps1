# Downloads the English-only quantized Whisper model (~60 MB) used by Vox.
$dir = Join-Path (Split-Path $PSScriptRoot -Parent) "models"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Invoke-WebRequest -Uri "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en-q5_1.bin" `
  -OutFile (Join-Path $dir "ggml-base.en-q5_1.bin")
