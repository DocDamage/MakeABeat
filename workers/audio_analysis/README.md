# Local audio analysis

The app runs the built-in pulse tracker in its own process. No Python is required for the fallback, manual editing, slicing, or warp rendering.

For Beat This!, create an isolated Python environment and install CPU PyTorch for that Python version. Then install the pinned package.

```powershell
python -m venv .venv-beats
# Install CPU PyTorch in this environment using the upstream platform instructions.
.\.venv-beats\Scripts\python.exe -m pip install beat-this==1.1.0
```

In **Studio → Sample → Beat map → Beat This! setup**, select the environment’s Python executable and check the environment. Use **Download verified weights** to install the 8.1 MiB `small0` checkpoint in application data. The app verifies its SHA-256 before running inference and passes the model an explicit local path. [Beat This! setup](https://github.com/CPJKU/beat_this#inference)

## Protocol v1

The host writes one versioned JSON `run` request. The built-in worker reads it from stdin. The Python adapter reads the same request from a job file, which avoids buffering in the Windows virtual environment launcher. The operation is `beats` or, for Beat This!, `probe`. The request supplies the job ID, project revision, optional source ID, fingerprint, original source frame range, sample rate, WAV input path, model ID and version, settings, and timeout. The WAV contains the requested interval.

Workers emit newline-delimited JSON progress and a terminal result or failure on stdout. Beat positions are seconds relative to the supplied WAV. The host adds the source offset after validating the response, including the request fields echoed by the result. Worker logs go to stderr. The built-in worker accepts a `cancel` message with the same version and job ID. The host enforces deadlines and terminates the Python process tree on Windows when a job is cancelled or times out.

The verified checkpoint has SHA-256 `6074be2c4d490c5f6101fcc374a1ec72ae93456e23bb6019783b849f5dc7d47b`. The app keeps up to 64 successful beat proposals in its analysis cache. The project stores a result only after the user applies it.

## Local CLAP sample search

Use a dedicated Python 3.10 or newer environment. The Windows benchmark used Python 3.10.11 and the CPU build of PyTorch.

```powershell
python -m venv .venv-clap
$clapPython = ".\.venv-clap\Scripts\python.exe"
& $clapPython -m pip install --index-url https://download.pytorch.org/whl/cpu torch==2.13.0
& $clapPython -m pip install transformers==5.14.1 numpy==2.2.6 scipy==1.15.3
```

In **Search → Index and model setup**, choose that Python executable and click **Download CLAP model**. Setup downloads and verifies the 615 MB weights for the pinned revision. Search and indexing load the snapshot from disk. They work offline after setup. You can also run the setup script explicitly from the repository root.

```powershell
& $clapPython workers/audio_analysis/clap_worker.py --setup
```

The model is the LAION unfused checkpoint at revision `8fa0f1c6d0433df6e97c127f64b2a1d6c0dcda8a`. Preprocessing averages stereo channels, uses polyphase resampling to 48 kHz and CLAP repeat padding, and stores normalized 512-value embeddings. Folder recordings use eight-second windows with four-second hops. Saved cuts use two-second windows with one-second hops. Short recordings are indexed whole and the final window reaches the end of the recording. The model, preprocessing and window version identify the embedding space in SQLite. [Pinned checkpoint](https://huggingface.co/laion/clap-htsat-unfused/tree/8fa0f1c6d0433df6e97c127f64b2a1d6c0dcda8a)

### Retrieval benchmark

Download the ESC-50 archive and optionally the NSynth test JSON/WAV archive. The prep script picks four labeled files per class deterministically and writes the text queries before retrieval. It writes the prepared audio to a folder outside the repository. [ESC-50 archive at the tested revision](https://codeload.github.com/karolpiczak/ESC-50/zip/33c8ce9eb2cf0b1c2f8bcf322eb349b6be34dbb6) · [NSynth dataset](https://magenta.withgoogle.com/datasets/nsynth)

```powershell
& $clapPython workers/audio_analysis/prepare_search_eval.py C:\audio-eval\esc50.zip C:\audio-eval\corpus --nsynth C:\audio-eval\nsynth-test.jsonwav.tar.gz
$env:LOST_SAMPLES_SEARCH_CORPUS = "C:\audio-eval\corpus"
$env:LOST_SAMPLES_SEARCH_PYTHON = (Resolve-Path $clapPython).Path
$env:HF_HUB_OFFLINE = "1"
cargo test --release --locked clap_retrieval_resume_and_warm_latency -- --ignored --nocapture
```

The test interrupts a real indexing run, resumes from its SQLite checkpoint, scores the labeled queries, and logs every miss. It pads the database to 10,000 passages with copies of measured embeddings for latency testing. That timing includes warm CLAP text embedding and exact SQLite scoring.

## Basic Pitch transcription

Use a dedicated Python 3.10 Windows x64 environment for the verified CPU setup. The Basic Pitch wheel includes the ONNX checkpoint. Installing the wheel without its automatic dependencies lets you select ONNX Runtime and its CPU audio dependencies directly.

```powershell
python -m venv .venv-pitch
$pitchPython = ".\.venv-pitch\Scripts\python.exe"
& $pitchPython -m pip install --no-deps basic-pitch==0.4.0
& $pitchPython -m pip install onnxruntime==1.23.2 librosa==0.11.0 numpy==2.2.6 scipy==1.15.3 pretty-midi==0.2.11 mir_eval==0.8.2 resampy==0.4.2
& $pitchPython -m pip check
```

In **Sample → Transcribe to notes → Basic Pitch setup**, choose that Python executable. The local worker verifies the included `nmp.onnx` checkpoint before inference. Its SHA-256 is `2c3c1d144bfa61ad236e92e169c13535c880469a12a047d4e73451f2c059a0ec`. The worker writes source-time note events and pitch contours to a temporary job directory. The app can cancel the process and removes its temporary audio when the job ends. [Basic Pitch](https://github.com/spotify/basic-pitch)

### Transcription benchmark

Use the official GuitarSet 1.1.0 annotations and mono microphone archives. The preparation script selects the first solo and accompaniment excerpt for each of the six players. It also synthesizes an isolated bass phrase and a mix containing that phrase plus drums and chords. [GuitarSet 1.1.0](https://zenodo.org/records/3371780)

```powershell
& $pitchPython workers/audio_analysis/prepare_pitch_eval.py C:\pitch-eval\corpus --annotations C:\pitch-eval\annotation.zip --audio C:\pitch-eval\audio_mono-mic.zip
$env:LOST_SAMPLES_PITCH_CORPUS = "C:\pitch-eval\corpus"
$env:LOST_SAMPLES_PITCH_PYTHON = (Resolve-Path $pitchPython).Path
cargo test --release --locked basic_pitch_evaluates_labeled_corpus -- --ignored --nocapture
```

## SAM Audio Windows feasibility probe

Use an isolated Python 3.11 environment for Phase 8. This setup uses the upstream SAM Audio source, CUDA 12.8 PyTorch wheels, and TorchCodec 0.10.0. TorchCodec needs the shared FFmpeg DLLs, which the standalone FFmpeg executable does not provide. Unpack the [FFmpeg 8.1.2 full shared build](https://www.gyan.dev/ffmpeg/builds/#release-builds) under the probe directory before running it.

```powershell
$samProbe = Join-Path $env:LOCALAPPDATA 'Lost Samples Desktop\sam-audio-phase8'
New-Item -ItemType Directory -Force $samProbe | Out-Null
$samSource = Join-Path $samProbe 'upstream'
git clone https://github.com/facebookresearch/sam-audio.git $samSource
git -C $samSource checkout bb4c6999d2677c7402360e426afc01ddfad6dce0
uv venv --python 3.11 (Join-Path $samProbe 'venv')
$samPython = Join-Path $samProbe 'venv\Scripts\python.exe'
uv pip install --python $samPython --index https://download.pytorch.org/whl/cu128 --index-strategy unsafe-best-match $samSource 'torch==2.10.0+cu128' 'torchaudio==2.10.0+cu128' 'torchvision==0.25.0+cu128' 'torchcodec==0.10.0' 'transformers==4.57.6' 'soundfile==0.14.0'
uv pip check --python $samPython
```

Request access to the [small checkpoint](https://huggingface.co/facebook/sam-audio-small) and wait for approval, then authenticate locally with a read token from the approved account. A successful sign-in alone can still return HTTP 403 for the checkpoint. The pinned checkpoint is about 5.1 GB. The upstream loader can fetch additional model dependencies during the first run.

```powershell
uvx --from huggingface-hub hf auth login --force
uvx --from huggingface-hub hf auth whoami
$samFfmpeg = Join-Path $samProbe 'ffmpeg-8.1.2-full_build-shared\bin'
$env:Path = "$samFfmpeg;$env:Path"
$samInput = Join-Path $samProbe 'probe-input.wav'
& (Join-Path $samFfmpeg 'ffmpeg.exe') -hide_banner -loglevel error -y -f lavfi -i 'sine=frequency=440:sample_rate=48000:duration=3' -f lavfi -i 'anoisesrc=color=white:amplitude=0.12:sample_rate=48000:duration=3:seed=8' -filter_complex 'amix=inputs=2:normalize=0' $samInput
& $samPython workers/audio_analysis/sam_audio_probe.py $samInput 'sine wave tone' (Join-Path $samProbe 'probe-output')
```

You can substitute a mono or stereo clip up to six seconds and a lowercase prompt describing the target. The six-second cap is for the probe. The text-only pass uses one candidate and skips loading the optional rankers and automatic span predictor. The script writes original, target, and residual mono WAVs at the model sample rate after validating their sample counts and finite values. It reports load time, inference time, and peak memory allocated by PyTorch on CUDA.

In the app, open **Studio → Sample → Extract sound · Experimental**. The app detects the environment and shared FFmpeg folder at the paths above. If you used different paths, choose them in **SAM Audio setup**. Run the standalone probe once to cache the model and T5 encoder before extracting in the app, which uses the local cache. With a waveform selection of 0.10 to 6 seconds, enter a short description and click **Extract selection**. Preview the three mono outputs and save the target, residual, or both as project sources.

### Extraction benchmark

The fixed six-second suite contains four controlled mixtures with bass guitar, flute, choir as backing vocals, and synthesized kick and snare as targets. It also includes two ChoraleBricks ensemble excerpts for listening. The manifest records the sources, edits, checksums, and attribution. Downloaded and derived audio stays in the local probe directory. [ChoraleBricks](https://www.audiolabs-erlangen.de/resources/MIR/2025-ChoraleBricks)

```powershell
$samCorpus = Join-Path $samProbe 'extraction-benchmark'
$samResults = Join-Path $samProbe 'extraction-results'
if (!(Test-Path (Join-Path $samCorpus 'manifest.json'))) {
    & $samPython workers/audio_analysis/sam_audio_benchmark.py prepare $samCorpus --cache (Join-Path $samProbe 'benchmark-sources') --ffmpeg (Join-Path $samFfmpeg 'ffmpeg.exe')
    if ($LASTEXITCODE -ne 0) { throw 'Benchmark preparation failed' }
}
New-Item -ItemType Directory -Force $samResults | Out-Null
foreach ($case in (Get-Content (Join-Path $samCorpus 'manifest.json') -Raw | ConvertFrom-Json).cases) {
    $mix = Join-Path $samCorpus "$($case.id)\mix.wav"
    & $samPython workers/audio_analysis/sam_audio_probe.py $mix $case.prompt (Join-Path $samResults $case.id)
    if ($LASTEXITCODE -ne 0) { throw "Extraction failed for $($case.id)" }
}
& $samPython workers/audio_analysis/sam_audio_benchmark.py score $samCorpus $samResults
```

For each controlled mixture, record wanted and unwanted gain independently for the target and residual. The scorer fits the output to both aligned reference stems and reports the remaining error relative to the wanted stem. It also checks the summed outputs against the input. For a listening pass, compare the original, target, and residual at matched levels. Rate isolation and damage separately for each prompt. Check whether the full wanted phrase is recognizable, whether the strongest competing instrument dropped, and whether the output introduced obvious artifacts or lost wanted notes. Report per-case failures and per-run latency and CUDA memory.
