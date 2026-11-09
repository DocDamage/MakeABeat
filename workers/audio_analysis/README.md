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
