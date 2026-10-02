# Downloads a Whisper model for Vox into ./models.
#   -Model base   English-only, ~60 MB, for the CPU build (default)
#   -Model large  large-v3-turbo, ~570 MB, for the GPU build
param([ValidateSet("base", "large")][string]$Model = "base")
$file = if ($Model -eq "large") { "ggml-large-v3-turbo-q5_0.bin" } else { "ggml-base.en-q5_1.bin" }
$dir = Join-Path (Split-Path $PSScriptRoot -Parent) "models"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Invoke-WebRequest -Uri "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$file" -OutFile (Join-Path $dir $file)
