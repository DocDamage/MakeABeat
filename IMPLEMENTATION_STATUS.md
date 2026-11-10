# Lost Samples Desktop implementation record

## Phase 0 compatibility and timing

The original checkout passed 43 tests with six ignored, and `cargo build --release --locked` passed. Clippy reported three existing warnings in `src/app.rs`. The repository-wide rustfmt check already reported differences.

The synthetic `LSFLIP01` and `LSFLIP02` files in `tests/fixtures/legacy` cover embedded single-source and multi-source audio, off-grid gate notes, swing, lane offsets, conditional ratchets, arrangement cycles, and volume automation. MIDI files and 16-bit stereo WAV checksums were captured from the original renderer. The regression test compares exports against those references. The mixer tests separately exercise external MIDI clock without requiring a hardware loopback.

Grid cells resolve their swung start, constrain their lane offset so all repeats remain in the same bar, and use the existing condition seed and repeat spacing. Recorded pad and key events enter at their authored ticks without applying swing or lane offsets again. Audio playback, live MIDI, WAV rendering, resampling, and MIDI-file export share resolved triggers. MIDI uses the legacy channel and velocity conventions. At a simultaneous grid tick in a project with choke groups, the pad triggers first, followed by the key and drums in lane order. Recorded hits follow the grid at that tick. The original scheduling order remains in projects without groups.

`LSFLIP03` marks choke groups, `LSFLIP04` marks persistent source identity or snapshots, and `LSFLIP05` marks persistent hit identities or sampler controls. The reader supports all five versions and rejects unsupported future markers. Opening a legacy project in the editor assigns IDs to its existing hits. Snapshot references translate source IDs back to runtime indices and share the project’s embedded audio. Moving and quantizing hits retains their IDs. Pasting and duplicating give the new hits their own IDs and sound controls.

## First release features

| Feature | Validation and limits |
| --- | --- |
| Retrospective capture | Tests cover stopped transport, loop wrap, sustain, external clock interruption, queue overflow, undo, save/reopen, and WAV and MIDI export. MIDI pitch bends are flagged in the preview and remain outside the saved take. |
| Choke groups | Tests cover simultaneous pad and drum hits, a 5 ms release in rendered output, persistence, and a routed drum stem that retains triggers from other tracks. |
| Pattern variations | Tests cover four seeded candidates, lane and cell locks, condition preservation, zero variation, and undo after Keep. Audition restarts at bar one on the internal clock. |
| Named snapshots | Tests cover source reordering after save/reopen and undo after Restore. Comparisons render up to four common bars and explicitly restart both sides at the selected bar. Level matching uses RMS. |

The Windows reference machine runs Windows 11 Pro build 26200 with an Intel Core i5-14600K and a GeForce RTX 3060 with 12 GiB of VRAM. The output tests used ARZOPA (NVIDIA High Definition Audio) at 48 kHz with a system-selected buffer of 480 frames. The short debug capture test measured a peak callback duration of 1.22 ms against a 10 ms buffer period. Both the on-device capture and level-matched A/B tests passed. Callback allocation profiling and release UI performance measurements remain open. The Phase 9 record below covers the 30-minute dropout run.

## Phase 2 sampler and DAW transfer

Pads, assigned keys, and the chromatic instrument support one base layer plus three additional layers. The mixer allocates layer slots in advance. Each hit selects layers by inclusive velocity ranges, and processed pad and key cuts share the mixer’s layer voices. The DAW manifest includes source mappings and velocity ranges. Chromatic mono playback uses the most recent active note, including sustained notes, and returns to the previous active note on release. Glide duration is measured in output frames. Per-hit pitch, sample-start offset, and cutoff apply independently of the instrument settings and travel through WAV, stems, and resampling. MIDI exports retain trigger notes and timing.

Windows drag export prepares a cached WAV in the background and uses the Shell file data object for OLE drag-out. The cache lives in the application data folder so imported sessions can retain their file paths. Raw selections, processed pads, and pattern renders are available. The Shell format and cache tests cover the native file payload, repeat preparation, and cancellation. The two-DAW drop and session-reopen gate remains open. [Windows drag API](https://learn.microsoft.com/en-us/windows/win32/api/ole2/nf-ole2-dodragdrop)

## Local model feasibility

The smoke tests used a six-second 120 BPM synthetic drum and bass loop and its isolated bass line. Measurements include peak process working set. These tests measured CPU inference and inspected output against known timing and pitches. A real-audio corpus and listening evaluation remain open.

| Candidate | Setup and result |
| --- | --- |
| Beat This! 1.1.0, `small0` | PyTorch 2.13 CPU. Cached startup 1.35 s, inference 65 ms, peak 243 MiB. Found 10 of the 12 in-range beats within 20 ms and all three in-range downbeats. It also emitted an event at the end boundary. [Upstream project](https://github.com/CPJKU/beat_this) |
| CLAP, `laion/clap-htsat-unfused` | Transformers 5.14.1, PyTorch CPU, 614 MB safetensors weights. Cached startup 4.94 s, one audio/text scoring pass 192 ms, peak 887 MiB. Ranked the drum-and-bass description ahead of two unrelated prompts. Passage retrieval and music checkpoint choice remain open. [Checkpoint](https://huggingface.co/laion/clap-htsat-unfused) |
| Basic Pitch 0.4.0, `nmp.onnx` | ONNX Runtime 1.23.2 on CPU. Startup 2.01 s, first inference including audio setup 2.19 s, peak 217 MiB. Recovered all 12 expected pitches near the expected onsets and returned 12 additional candidates. [Upstream project](https://github.com/spotify/basic-pitch) |
| SAM Audio | The base checkpoint requires approved access and authenticated download. The test environment’s PyTorch build has CPU support only. The upstream setup requires Python 3.11 or later and recommends CUDA. Inference, Windows compatibility, and redistribution clearance remain open. [Model access](https://huggingface.co/facebook/sam-audio-base) · [Setup](https://github.com/facebookresearch/sam-audio) · [License](https://github.com/facebookresearch/sam-audio/blob/main/LICENSE) |

## Analysis worker protocol v1

Use newline-delimited JSON on stdin and stdout. Reserve stderr for diagnostic logs. The host creates a per-job directory and provides the worker an input WAV path and output directory. Paths in results are relative to the job directory. The host validates them and the result schema before attaching any output to a project.

- A `run` request contains `version`, `job_id`, `project_revision`, `source_id`, `source_fingerprint`, `start_frame`, `end_frame`, `sample_rate`, `input_path`, `output_dir`, `operation`, `model_id`, `model_version`, `settings`, and `timeout_ms`. Use a half-open range in source frames and a SHA-256 fingerprint of the canonical decoded audio including its rate and channel format.
- A `progress` response contains `version`, `job_id`, `stage`, `completed`, and `total`. A `cancel` request identifies the job by `version` and `job_id`. The host may terminate a worker that misses its cancellation grace period or deadline.
- A terminal `result` response echoes the job ID, project revision, source fingerprint, range, and model version and returns a versioned operation payload and artifact list. A terminal `failure` response includes a stable code and user-facing message. Use distinct codes for cancellation, timeout, unavailable model, unsupported device, invalid audio, and inference failure.
- Cache keys include the source fingerprint, frame range, operation, model and preprocessing versions, and canonical settings. The host may cache a valid stale result. Only a result matching the current project revision, source identity, and pending operation can modify the project.

The built-in pulse tracker runs in a child process of the desktop executable. Beat This! uses the selected isolated Python environment and the verified local `small0` checkpoint. The host validates replies and rejects stale results by project revision, source buffer, and selection. An independent bounded queue prioritizes interactive previews over queued analysis and can yield indexing work when Phase 5 uses it. Recovery saves remain available during analysis.

## Phase 3 beat maps and warp

`LSFLIP06` stores confirmed beat maps, anchors, first downbeats, editable phrase starts, and source provenance for accepted warps. Unaccepted suggestions are cached outside the project by audio fingerprint, range, model version, and settings. The waveform distinguishes confirmed beats and pending suggestions. Slices use the confirmed beat or bar positions. The warp preview and committed source share the same audio buffer. The 0.5× to 2× stretch constraint remains in effect.

The Windows synthetic timing comparison used 12-second drum clips and a one-to-one 70 ms match tolerance. Times include process startup in a debug build for the pulse tracker and a CPU Python environment for Beat This!.

| Clip | Built-in pulse tracker | Beat This! small0 |
| --- | --- | --- |
| Steady | 24/24 matched, 0 extra, 0.23 s | 24/24 matched, 0 extra, 1.70 s |
| Gradual drift | 19/24 matched, 4 extra, 0.22 s | 22/24 matched, 4 extra, 1.56 s |
| Pickup | 22/22 matched, 2 extra, 0.21 s | 22/22 matched, 2 extra, 1.65 s |
| Sparse | 24/24 matched, 0 extra, 0.23 s | 24/24 matched, 0 extra, 1.59 s |

The supervised Windows Beat This! test passed Python environment validation, checkpoint download, SHA-256 verification, and CPU inference. Tests also exercise malformed protocol requests, subprocess cancellation, queue priority and resumption, manual corrections across reanalysis, and cancellation or stale results before a warp is attached. Synthetic stretch tests check pitched vowel-like tones, sustained chords, and drifting drum transients. Real-recording listening and stretch-quality evaluation remain open, along with the Phase 2 two-DAW acceptance gate.

## Phase 4 groove transfer and beat audition

`LSFLIP07` stores reusable groove templates and per-hit grid feel. Groove extraction uses one to eight complete mapped bars, with corrected beat anchors and local transient evidence for sixteenth-note positions. It normalizes the measured peak accents around 1.0× and leaves unobserved accents neutral. Timing offsets are bounded to ±30 ticks. The synthetic regression recovers its programmed offsets within two ticks and normalized accents within 0.04. Tests also cover independent strengths, lane scope, recorded-performance opt-in, one-step undo, and an early drum onset shared by WAV and MIDI after reopening the project. Legacy WAV and MIDI fixtures still match.

Queue candidates, including the existing sound matches, and saved cuts can audition against the active beat. Preparation runs on a background worker. A four-entry render cache keys by decoded audio fingerprint, effective range, beat map, destination tempo, and bar settings. The mixer starts each replacement at a bar boundary and keeps fitted loops aligned on later passes. It mixes audition audio at a separate level. Internal tempo changes invalidate the render. External clock playback uses original speed and clears the audition on a position jump or restart.

The deterministic mixer test joined on the exact downbeat frame while the drum pattern continued. The Windows device test repeatedly auditioned a saved cut, replaced the fit after a tempo change, and verified that the previews created no project or undo changes. The shared audition entry point now also serves semantic-search results. Phase 4 listening checks on real mixed recordings remain open.

## Phase 5 local sample search

The Search page indexes user-selected folders, saved cuts, and recordings in the existing YouTube cache. SQLite tracks file size and modification time, decoded PCM fingerprints, provenance, source timestamps, and model-specific passage checkpoints. Rescanning skips unchanged completed files, invalidates changed or missing paths, and reuses embeddings for identical audio. The index is bounded to 10,000 passages, a 160 MiB SQLite database, and a 128 MiB saved-cut audio cache. Search and result actions use local audio. Model download and verification run only from the explicit setup action.

Text search uses exact cosine scoring with the pinned LAION CLAP checkpoint and fixed mono 48 kHz preprocessing. **Find similar locally** also uses the existing rhythm, brightness, energy, and texture features. Results merge nearby overlapping windows, deduplicate exact passage fingerprints, and offer beat audition and project import actions. [Pinned model revision](https://huggingface.co/laion/clap-htsat-unfused/tree/8fa0f1c6d0433df6e97c127f64b2a1d6c0dcda8a)

The offline evaluation used 240 labeled recordings across 60 fixed queries. The index resumed after 12 passages had been committed and reached 240 without restarting those passages. On an Intel Core i5-14600K running Windows, warm p95 was **113.0 ms** at 10,000 stored passages, including text embedding. The latency test used repeated embeddings to fill the 10,000-passage database. [ESC-50](https://github.com/karolpiczak/ESC-50) · [NSynth](https://magenta.withgoogle.com/datasets/nsynth)

| Query set | Top-five matches |
| --- | --- |
| ESC-50 field sounds | 48/50 |
| NSynth instruments | 8/10 |
| Combined | 56/60 (93.3%) |

| Miss | Returned labels in rank order |
| --- | --- |
| Engine | Washing machine, helicopter, helicopter, helicopter, vacuum cleaner |
| Hen | Cow, train, thunderstorm, crackling fire, crickets |
| Low bass instrument note | Strings, organ, reed, keyboard, reed |
| Keyboard playing one musical note | Mouse click, organ, guitar, organ, flute |

A timing check placed 12 labeled environmental sounds and instrument notes into four 24-second montages at 1.25, 9.5, and 17.75 seconds. A result counted when the window overlapped at least half of the target sound. Eight-second windows with four-second hops found 12/12 in the top five and 9/12 at rank one. Four-second windows with two-second hops found 11/12 in the top five and 7/12 at rank one. The longer windows remain the default for recordings. The benchmark covers field sounds, instrument notes, and timed montages. Retrieval quality on real mixed drum breaks and complete songs remains open.

## Phase 6 taste feedback

**Queue → Taste profile** is off by default. It reranks the first 96 pending tracks already available in the Queue and preserves the original view for cold starts, for **Similar matches only**, and when the user switches personalization off. Ranking reads version-matched Phase 5 vectors from the local index and uses exact catalog matches when available. It shows the kept or skipped recording and matching signals for each scored item. The Familiar–Adventurous slider increases the penalty for overlap with earlier results. Every fourth position gives extra weight to variety.

The crate saves each explicit decision and its feedback in the same write. Undo restores the last 32 decision batches across app launches. Reset clears training feedback and leaves the queue verdicts available for ordinary undo. A Skip contributes only after two seconds of output playback. Contextual skips, unheard skips, Later, and playback alone contribute no negative signal. The ranking uses up to 64 recent keeps and 64 eligible skips.

The offline check held out two recordings for each of 40 simulated feedback sessions using the Phase 5 NSynth subset. Two labeled examples supplied the Keep decisions, one example from another class supplied a Skip, and catalog labels were hidden from the ranker. Mean nDCG@5 was **0.106** for the original candidate order, **0.711** for Familiar, and **0.542** with the slider at 80% Adventurous. This measures retrieval of same-family instrument examples under simulated verdicts. Listening sessions with real preferences and mixed recordings remain open before any change to the default. [NSynth dataset](https://magenta.withgoogle.com/datasets/nsynth)

Run the held-out check with the Phase 5 benchmark corpus and Python settings.

```powershell
cargo test --release --locked taste_ranking_on_held_out_nsynth_feedback -- --ignored --nocapture
```

## Phase 7 note transcription

The app runs Basic Pitch 0.4.0 with CPU ONNX Runtime 1.23.2 in a supervised local Python process. A clean Windows x64 Python 3.10 environment passed dependency validation and inference. The ONNX checkpoint ships in the Python wheel and the worker verifies its SHA-256. This reuses the upstream detector and postprocessing without bundling a Python runtime or reimplementing model preprocessing in Rust. [Basic Pitch](https://github.com/spotify/basic-pitch)

`LSFLIP08` stores per-note instrument choices and pitch curves. Transcription maps through the corrected beat map when available and otherwise uses the entered source tempo and alignment. Import creates an editable pattern in one undo step. Longer phrases use linked patterns and arrangement sections, with continuations joined across boundaries in song playback and exports. WAV renders preserve bends. MIDI export can allocate 13 independent bend channels, set the ±12-semitone range, and reset each channel after use. The quantized export uses semitone pitches and the ordinary keys channel. Live MIDI uses the ordinary keys channel. A Mido import confirmed the generated per-note MIDI channels and standard keys channel.

The offline evaluation used default thresholds on synthetic bass and 12 annotated GuitarSet microphone recordings, one solo and one accompaniment per player. Onset matches required the same MIDI pitch within 50 ms. Offset matches also required the end to land within 50 ms or 20% of the reference note duration, whichever was larger. Mean errors are calculated over onset-and-pitch matches. [GuitarSet 1.1.0](https://zenodo.org/records/3371780)

| Input | Onset and pitch matches | Candidates | F1 | Offset matches | Onset MAE | Duration MAE |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Isolated bass | 11/12 | 12 | 0.917 | 10 | 10.6 ms | 45.6 ms |
| Bass with drums and chords | 11/12 | 33 | 0.489 | 10 | 17.7 ms | 52.8 ms |
| GuitarSet solos | 301/333 | 403 | 0.818 | 263 | 11.6 ms | 87.4 ms |
| GuitarSet accompaniment | 674/824 | 969 | 0.752 | 457 | 11.4 ms | 83.8 ms |

The dense-mix evaluation labels only the bass target, so backing notes count against precision. Isolating a stem and checking extra candidates and note lengths remains important.

## Phase 8 extraction by description

The native Windows 11 probe installed upstream SAM Audio at `bb4c6999d2677c7402360e426afc01ddfad6dce0` in an isolated Python 3.11.15 environment. PyTorch 2.10.0 with CUDA 12.8 ran on the 12 GiB RTX 3060. TorchCodec 0.10.0 decoded a three-second 48 kHz WAV using the FFmpeg 8.1.2 shared DLLs. The upstream SAM Audio imports and prompt processor passed, including moving the batch to CUDA. xFormers reported that Triton optimizations were unavailable during import. [Probe setup](workers/audio_analysis/README.md#sam-audio-windows-feasibility-probe)

After Hugging Face granted access, the pinned 5.1 GB small checkpoint downloaded and matched its published SHA-256. The cached checkpoint and T5 encoder then ran offline on native Windows with one text-prompt candidate. The three-second tone-plus-noise probe produced aligned original, target, and residual mono WAVs at 48 kHz. Load took 9.06 s, inference took 1.84 s, and peak PyTorch CUDA allocation was 6,459 MiB. The 440 Hz tone amplitude was 0.12493 in the original, 0.09361 in the target, and 0.00014 in the residual. Redistribution clearance remains open. [Checkpoint](https://huggingface.co/facebook/sam-audio-small) · [SAM License](https://github.com/facebookresearch/sam-audio/blob/bb4c6999d2677c7402360e426afc01ddfad6dce0/LICENSE)

The extraction benchmark fixes six six-second prompts before the first model run. Four controlled mixes have aligned target and residual references for bass guitar, flute, backing vocals, and kick and snare. Two recorded wind and brass ensembles are reserved for listening. Preparation verified the original file checksums and all six WAVs. Scoring was checked with reference stems and a mixture passthrough to verify that it distinguishes leakage from target gain. [Benchmark instructions](workers/audio_analysis/README.md#extraction-benchmark) · [ChoraleBricks recordings](https://www.audiolabs-erlangen.de/resources/MIR/2025-ChoraleBricks)

All six cases ran offline and returned finite mono outputs of exactly 288,000 frames at 48 kHz. Each case loaded the cached model in a fresh process. Loads took 8.35–8.82 s, inference took 2.36–2.44 s, and peak PyTorch CUDA allocation was 7,251 MiB. The text-only run used one candidate and skipped the optional rankers and span predictor.

| Prompt | Target gain against wanted reference | Leakage against backing reference | Target error relative to wanted reference |
| --- | ---: | ---: | ---: |
| Bass guitar | −0.72 dB | −36.71 dB | −17.75 dB |
| Flute | −5.98 dB | −60.65 dB | −6.86 dB |
| Backing vocals | −4.84 dB | −42.72 dB | −5.86 dB |
| Kick and snare drums | −3.89 dB | −54.37 dB | −5.28 dB |

These gains are waveform projections on the two reference stems. Timing or phase differences can reduce the measured gain or increase the error. The bass case had the smallest reference error. The other controlled cases had larger errors despite low measured backing leakage. Listening on both ensembles and comparison with Demucs remain open.

The experimental Sample panel runs SAM Audio offline in a cancellable worker for selections of 0.10 to 6 seconds. It previews the mono original, target, and residual before adding any audio to a project. Saving either output or both embeds audio and provenance in `LSFLIP09`. The Windows integration test cancelled one run, reran the model on a 44.1 kHz source, played all three previews on the output device, saved both in one undo step, and verified undo, redo, and reopen.

## Phase 9 Windows audio soak

Run `cargo test --release --locked windows_audio_dropout_soak -- --ignored --nocapture` on a Windows machine with working output and loopback. The test plays a quiet beat and a 6 kHz pilot through the default device. Keep the output volume steady and pause other audio on that endpoint. Each background cycle exports and decodes a WAV, estimates its beat grid, renders a tempo-fitted preview, saves a project, and scans a sample folder into SQLite. Set `LOST_SAMPLES_SEARCH_PYTHON` to a prepared CLAP environment to also index embeddings.

The 30-minute release-mode run completed 120 cycles on Speakers (2- AIR 192 6) at 44.1 kHz with 441-frame callbacks. The test recorded 179,992 loopback windows, with 187 below a quarter of the initial pilot amplitude and nine timestamp gaps over the expected capture period plus 5 ms. One output callback exceeded its 10 ms buffer budget. Peak callback time was 10.572 ms. The dropout gate failed. The loopback flags show signal or capture interruptions and do not establish the cause or a count of audible dropouts. This run used folder scanning without CLAP embeddings. The SAM Audio checkpoint downloaded during the run, and inference started after playback ended.

The diagnostic harness adds a continuous 4.7 kHz reference alongside the existing 6 kHz audition pilot. It measures both signals immediately before WASAPI and in loopback. Thresholds are one quarter of the warmed-up median per band. The harness logs capture timestamp and render callback gaps separately, with stage timings and thread cycle counts for callbacks that overrun. The diagnostic hooks compile only in Windows test builds.

A 22-second diagnostic run on the same AIR 192 6 reproduced a loopback dip in both bands when the interval between render callbacks reached 29.960 ms. Both bands stayed above threshold in the audio sent to WASAPI. The mixer callback peaked at 1.014 ms and had no overruns. This reproduces an interruption between scheduled render callbacks. The render thread reported Windows priority 0. CPAL 0.15.3's WASAPI priority helper casts a thread ID to a handle before calling `SetThreadPriority`, which requires a thread handle. A separate Windows API check confirmed that the ID-as-handle lookup returned `ERROR_INVALID_HANDLE`. The original 10.572 ms callback and the cause of the capture timestamp gaps remain open. [CPAL WASAPI source](https://github.com/RustAudio/cpal/blob/ac6cbb2ba55e61665a35ab88ae136a83380d1354/src/host/wasapi/stream.rs#L303-L312) · [Windows thread priority API](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-setthreadpriority)

The first CLAP preflight exposed an offline load issue. The current Hub library validates that every requested snapshot file is cached. Setup downloads only the eight CLAP runtime files, while offline load was also requesting repository metadata. Both paths now use the same file list. The soak changes the drum pattern on each render so repeated tempos cannot reuse old embeddings. With CLAP enabled, it waits two seconds between cycles and verifies that each index pass commits new passages.

The 30-minute rerun completed 804 workload cycles and indexed 1,208 passages, spending 117.16 s in the indexing stage. It used the same AIR 192 6 endpoint at 44.1 kHz with 441-frame callbacks. There were zero callback overruns and the peak was 5.489 ms. All 179,993 captured windows retained both test signals. The harness flagged 604 extended intervals between render callbacks and seven loopback capture timestamp gaps. The longest capture gap exceeded the expected interval by 27.991 ms. The soak failed its zero-gap criterion. The timestamps and waveform measurements do not establish whether playback remained continuous during the missing capture intervals.

The Windows playback and recording callbacks now apply CPAL's intended time-critical priority once on their dedicated stream thread, using the handle returned by `GetCurrentThread`. The WASAPI loopback harness uses the same helper and checks both thread priorities before collecting data. The 25-second CLAP preflight confirmed priority 15 on both threads and passed with 18 new passages and zero capture gaps. [Windows current thread handle](https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getcurrentthread)

The full 30-minute CLAP soak passed after the priority fix. It used the same AIR 192 6 endpoint at 44.1 kHz with 441-frame callbacks. The workload completed 806 cycles and indexed 1,212 passages, spending 115.37 s in the indexing stage. All 180,001 captured windows retained both test signals. Capture timestamp gaps fell from seven to zero, and extended intervals between render callbacks fell from 604 to 15. Peak callback time fell from 5.489 to 1.925 ms, with zero overruns in either CLAP run.

### Windows release verification on 2026-11-09

The 30-minute soak on commit `75ba98f` used Rust 1.99.1 on Windows 11 Pro (build 26200), the AIR 192 6 endpoint at 44.1 kHz, and 441-frame callbacks. Both stream threads reported Windows priority 15. The workload completed 799 render, analysis, save, and scan cycles and indexed 1,199 CLAP passages in 126.94 s of indexing.

**The soak failed the loopback reference gate.** Of 180,001 loopback windows, 504 had a weak reference, 266 had a weak audition pilot, and 10 had both. The counters stopped increasing by the 15:30 checkpoint. The render probes recorded zero weak windows before WASAPI and zero callbacks over the 10 ms buffer budget, with a peak of 0.649 ms. Loopback capture reported zero timestamp gaps. The test recorded six extended intervals between render callbacks, with a longest interval of 20.323 ms. The cause and audibility of the loopback dips remain undetermined.

The separate native UI probe used a temporary instrumented release build of the same commit and isolated application data. It exercised the Sequence grid during CLAP indexing of 530 distinct seven-second WAVs, persistence of a 1,000-record crate (67.8 MiB on disk), and recovery and portable saves of a project containing 90 seconds of stereo audio. A background timer queued pointer events for the native egui loop, with extra events triggered at the start of recovery writes. Latency ran from enqueue to the end of the app update that changed the drum step. It excludes physical input and display presentation.

| Concurrent work | Completed jobs | Edits that overlapped the work | Max edit latency | Max gap between app updates |
| --- | ---: | ---: | ---: | ---: |
| CLAP indexing | 1 | 507 | 17.2 ms | 32.8 ms |
| Crate persistence | 23 | 75 | 17.2 ms | 22.9 ms |
| Recovery writes | 3 | 4 | 16.9 ms | 17.3 ms |
| Portable project saves | 3 | 3 | 4.6 ms | 27.7 ms |

All 590 edits changed the drum step, with a 13.6 ms p95 and a 17.2 ms maximum. Of the four edits that overlapped recovery writes, two reached project state before the write completed. Across 4,142 app updates, the longest update took 5.7 ms and the longest gap between update starts was 32.8 ms. Both stayed below the 100 ms target. The index finished with 530 passages and zero pending or failed files.

### WASAPI loopback interference follow-up

WASAPI endpoint loopback captures the shared output mix. During the failed soak, some weak audition windows had a reference tone several times stronger than calibration. A controlled 13-second run on the same AIR 192 6 reproduced the failure by playing two quiet tones near the test frequencies from a second process after calibration. The original detector recorded 64 weak reference windows with zero capture timestamp gaps, callback overruns, or weak render windows. This establishes that competing audio can produce the recorded failure pattern. The process responsible for the earlier dips could not be identified from the previous logs. [Microsoft WASAPI loopback documentation](https://learn.microsoft.com/en-gb/windows/win32/coreaudio/loopback-recording)

The harness now samples the default endpoint's audio session meters during calibration and capture. If it detects another process above 0.00004 full scale after session gain, the test reports the process ID and peak and marks the endpoint run inconclusive. The controlled run with the guard again recorded 64 weak reference windows and identified the test stream's process. On a quiet endpoint the original continuity checks still apply. The guard runs outside the audio callbacks.

**The guarded 30-minute release soak passed** on the AIR 192 6 at 44.1 kHz with 441-frame callbacks. The guard detected no competing audio and all 180,001 loopback windows retained both pilots. The run recorded zero capture timestamp gaps, zero weak windows before WASAPI, and zero callbacks over budget. Peak callback time was 0.297 ms, with three extended render callback intervals. The workload completed 809 cycles and indexed 1,217 CLAP passages in 111.62 s of indexing.
