This plan covers every improvement proposed for Lost Samples Desktop: retrospective performance capture; groove extraction; smarter chopping and warping; choke groups, sample layers, bass glide, and per-step sound controls; constrained pattern variations; semantic sample search; personalized digging; extraction by description; audio-to-MIDI; tempo-matched auditioning; named A/B snapshots; and dragging rendered chops into a DAW.

Prepared November 8, 2026. The [implementation record](IMPLEMENTATION_STATUS.md) tracks implemented features, validation, and open gates. The plan assumes Windows is the first supported delivery target, processing stays local by default, and the existing Rust/egui/CPAL application remains the product. Complexity below is relative scope, not a delivery-date promise. Model quality and hardware requirements must be measured before scheduling their production releases.

The first release should deliver capture, choke groups, variations, and snapshots. Subsequent releases add deeper sampler controls, musical analysis, discovery, transcription, and experimental extraction. Each phase must produce an independently usable feature set.

| Phase | Deliverable | Prerequisites | Complexity |
| --- | --- | --- | --- |
| 0 | Compatibility, timing contracts, and baseline measurements | Existing app | Medium |
| 1 | Capture, choke groups, variations, named snapshots | 0 | Large |
| 2 | Layers, glide, per-step controls, DAW drag-out | 0–1 | Large |
| 3 | Analysis worker, editable beat maps, smart chops and warp | 0 | Large |
| 4 | Groove extraction and tempo-matched audition | 2–3 | Large |
| 5 | Local audio index and semantic search | 3 worker infrastructure | Large |
| 6 | Personalized digging | 5 | Medium |
| 7 | Audio-to-MIDI editing and replay | 2–3 | Large |
| 8 | Extraction by description | 3 worker infrastructure; early feasibility result | Research, then large integration |
| 9 | Integrated validation, packaging, and documentation | All preceding release gates | Medium |

The preferred delivery order is the table order. Phases 3 and 5 do not require every sampler enhancement to finish. Conduct a short extraction feasibility study during Phase 0 so its uncertainty is known early; ship it only after its later quality gate. These dependencies describe the work, not a requirement to run multiple agents.

The current code already supplies useful foundations. `src/audio.rs` owns the mixer, live commands, and captured performance events; `src/model.rs` owns patterns and source assignments; `src/app/project_session.rs` supplies undo and recovery; and `src/files.rs` plus `src/daw.rs` handle persistence and rendering. `src/analysis.rs` currently compares seven acoustic features, `src/tempo.rs` estimates a single tempo, `src/stretch.rs` renders pitch-preserving stretches with optional markers, and `src/separation.rs` launches Demucs. Keep these working while introducing small feature modules. Extract code from `src/app.rs` and `src/ui.rs` only where needed for a feature.

**Phase 0 establishes the contracts that every feature must follow.**

Before implementation, capture representative legacy projects, rendered references, and the current test/build results. Include single-source and multi-source projects, recorded notes, external clock, automation, conditional steps, and arrangements. Record the reference Windows machine, audio device, buffer size, and sample rate for performance comparisons.

Introduce a shared resolved-event representation for playback and exports, without immediately replacing the existing step-grid editor. It must represent drum/pad/key identity, onset, duration, velocity, sound overrides, and stable note identity. Add expressive fields incrementally when their feature is implemented. The current `PlayedChop` model covers pads and keys; groove on drums needs an explicit drum timing path too. Stable note IDs must survive copy, paste, quantize, undo, and pattern duplication without sharing mutable overrides accidentally.

Define the timing order once: base event time, optional selected groove transformation, lane offset, conditions/repeats, then scheduling. Specify bar-boundary handling and deterministic ordering of simultaneous hits. Existing projects must retain their current swing, lane offsets, recorded timing, and condition behavior. Do not apply swing a second time to timing that already includes it. Rendering, resampling, live MIDI, and MIDI-file export must consume the same resolved events where applicable.

Version persistent changes explicitly. Preserve reading `LSFLIP01` and `LSFLIP02` files. Use a new file-format marker when a project includes semantics that older binaries would silently discard. Add defaults and migrations for new fields, and reject unsupported future formats clearly. Introduce stable source IDs with legacy index translation at the boundary; retain the current runtime lookup where practical. Snapshots and analysis results must keep referring to the same audio after source-list changes.

Separate user-authored data from disposable analysis caches. Edited beat maps, saved grooves, accepted notes, and snapshots belong in the project. Embeddings, model weights, preview renders, and rebuildable machine analysis belong in application storage. Audio shared by project snapshots should be embedded once. Account for current project audio and metadata limits before adding sources or retaining snapshots.

All new audio-callback work must be bounded. Allocate buffers and compile playback data off the callback, use bounded nonblocking transport for capture, and retire replaced data away from the callback where needed. File operations, model inference, and native drag operations stay outside it. Profile the touched paths rather than assuming the existing engine is already free of allocations.

Define a versioned analysis-job contract now: job ID, project revision, source fingerprint, range, model/version, settings, progress, cancellation, and result. Implement the actual worker when Phase 3 begins. Stale results may enter the cache but cannot overwrite a newly edited project.

Early feasibility tasks: evaluate one short musical excerpt with each proposed model; record setup, latency, peak memory, and audible usefulness. For extraction, first check checkpoint access, supported hardware, and redistribution terms. If access or hardware is unavailable, record the dependency and continue the other work; feasibility has not passed merely because an adapter exists.

Primary touchpoints: `src/model.rs`, `src/audio.rs`, `src/files.rs`, `src/daw.rs`, `src/midi_out.rs`, and `src/app/project_session.rs`. Proposed modules: `src/model/events.rs` and `src/model/project_version.rs`.

Acceptance: legacy fixtures load and retain their playback behavior; resolved-event tests agree across render/export routes; undo and recovery preserve new defaults; baseline measurements and the model feasibility findings are recorded.

**Phase 1 ships a first release that protects ideas and encourages variations.**

- Retrospective capture: keep a bounded in-memory history of pad/key note-on, note-off, velocity, sustain, and relevant expressive events as they reach playback. Offer Capture last 4, 8, or 16 bars, with Capture to new pattern as the default destination. Capture works while Record is off and while the transport is stopped; stopped transport uses the current tempo and a visible alignment preview. Original timing is retained, with optional quantization at acceptance.
- Capture timing: store monotonic audio-frame timestamps plus transport/tempo information, rather than wrapping pattern positions alone. Begin a new capture segment on project changes, source/assignment changes that invalidate playback, device resets, and clock discontinuities. Show how much compatible history remains. Trim held notes at the capture boundaries and handle missing note-offs without stuck voices. Capture internal performances only; microphone history is outside this feature.
- Choke groups: assign pads and applicable drum sounds to an optional group; a new hit releases existing group voices using a short ramp. Define same-timestamp ordering and apply it consistently in live playback, song rendering, and resampling. Unassigned groups preserve old behavior.
- Pattern variations: lock lanes, individual hits, or a selected region; choose density, variation amount, and fill frequency; generate four candidates using reproducible seeds. Start with hats and percussion, then fills and chop rearrangement. Use musical rules and existing templates, with no inference requirement. Candidate audition must preserve the original pattern; Keep creates a named variation in one undo operation. Locked notes and their conditions remain unchanged.
- Named A/B snapshots: save a name, timestamp, musical project state, and referenced source IDs. Snapshot payloads cannot recursively include the snapshot collection or duplicate embedded audio. Compare pattern or song from the same musical position; use an explicit matched-level audition option. Keep the working state available until Restore, which is undoable. Preserve sources referenced by snapshots, enforce a visible storage/count limit, and persist snapshots across reopen.

Preview switching must flush or reconcile held voices and outgoing MIDI note-offs. Start with stopped restore and bar-boundary comparison for compatible playback states; explicitly restart previews with differing tempos or source layouts. Auditioning a candidate or snapshot cannot dirty the project by itself.

Primary touchpoints: `src/audio.rs`, `src/app.rs`, `src/model.rs`, `src/ui/step_grid.rs`, `src/ui/piano_roll.rs`, and `src/app/project_session.rs`. Proposed modules: `src/performance_capture.rs`, `src/model/variation.rs`, `src/model/snapshot.rs`, and focused UI modules under `src/ui/`.

Acceptance: capture survives loop wrap, long holds, sustain, stopped playback, and external-clock interruption; overflow is visible and bounded; locked content is unchanged; seeds reproduce candidates; choke behavior agrees in live and exported audio; snapshots reopen with the correct sources; every accepted change can be undone.

**Phase 2 expands sampler expression and makes DAW transfer immediate.**

- Sample layers: support up to four layers per pad or assigned instrument, with independent source range, gain, pan, tuning, and velocity range. Allow simultaneous layers and velocity switching. Existing assignments migrate to one layer. Show layers in the selected-trigger inspector and retain kit/project portability.
- Bass glide: add mono/legato mode, glide time, and retrigger behavior to chromatic instruments. Make the pitch transition independent of sample rate and define note priority and sustain behavior. Keep polyphonic playback as the legacy default.
- Per-step controls: let a hit override pitch, filter cutoff, and sample-start position. Define each override as relative to the instrument or absolute as appropriate, show its unit, and clamp it safely to the assigned range. Overrides affect that event's voices, not the shared instrument setting. Copy, paste, duplicate, variations, and undo must retain them.
- DAW drag-out: prepare a rendered WAV for a selected cut, pad, or pattern in a background job, then expose a ready-to-drag handle. Default processed exports include layers, glide, overrides, and tails; raw cuts remain available. Cache by source/settings fingerprint. Keep dragged files in a durable application export location so a DAW that references the path can reopen its session later. Provide explicit cleanup and Reveal in folder.

Audit layered stem rendering carefully: muting other tracks before render can change choke or voice-allocation behavior. Render the complete event context and route the requested output so individual stems retain the full mix's articulation. Include layer/source mappings in the DAW manifest. MIDI cannot encode arbitrary audio filtering, layering, or sample-start changes: preserve trigger notes and timing, identify unsupported sound controls in the import guide, and deliver rendered audio for fidelity.

Use a small Windows platform adapter for drag-out, with a prototype against actual DAW drop targets before product UI work. Windows provides the native OLE drag operation through [DoDragDrop](https://learn.microsoft.com/en-us/windows/win32/api/ole2/nf-ole2-dodragdrop); its initialization, drag-source interfaces, and event-loop behavior need integration testing with the app's window. Keep platform code behind conditional compilation.

Primary touchpoints: `src/model.rs`, `src/audio.rs`, `src/ui.rs`, `src/ui/piano_roll.rs`, `src/ui/step_grid.rs`, `src/daw.rs`, and `src/files.rs`. Proposed modules: `src/model/instrument.rs`, `src/audio/voice.rs`, and `src/platform/drag_export.rs`.

Acceptance: old single-layer projects remain equivalent; velocity boundaries and mono note priority are deterministic; simultaneous notes do not leak overrides; processed cuts, stems, and full mixes preserve articulation; canceled drags leave no incomplete file; successful imports reopen in two representative Windows DAWs. Record the DAW names and versions actually tested.

**Phase 3 adds an analysis worker and editable musical beat maps.**

Run inference in a supervised local process. Use a versioned JSON request/result protocol with file-based audio exchange, structured failures, progress, cancellation, and timeouts. Keep model environments isolated when dependencies conflict; a shared protocol does not require one shared Python environment. Show model installation/download size and device availability before use. Cache weights locally, verify downloads, and support offline operation after setup. Model provisioning happens through a user action, not as an unexpected app-startup download.

Replace the single global busy slot for these tasks with a small priority queue: interactive preview first, then foreground analysis, then resumable indexing. Limit expensive inference to one job initially. Cancel or yield indexing when necessary, keep autosave available, and isolate worker crashes from playback and project state. Retain the existing job path while migrating its callers incrementally.

Introduce a per-source BeatMap with beat timestamps, bar starts, user anchors, analysis provenance, and uncertainty information when supported. Keep the source's variable beat positions separate from the project's fixed playback tempo. Evaluate [Beat This!](https://github.com/CPJKU/beat_this), which provides beat/downbeat inference, against the current estimator on the reference corpus. Use the existing estimator as a lightweight fallback; manual correction must work with or without a model.

Expose the map over the waveform with controls for first downbeat, half/double tempo interpretation, adding/moving/removing beats, and resetting analysis. Preserve user anchors when rerunning analysis. Start phrase suggestions with bar-aligned lengths and transient/silence evidence; phrase boundaries are editable suggestions rather than guaranteed musical structure recognition.

Use the corrected map for slice-by-beat/bar and automatic warp-marker proposals. Preview before creating a new source. Maintain pitch preservation and original-source provenance. Benchmark the current stretcher on drums, vocals, sustained chords, and vinyl-like drift; evaluate a replacement backend only if listening tests show it is necessary. Keep the present 0.5–2.0 stretch constraint until a tested backend supports a wider range. Explain invalid segments rather than silently clipping marker positions.

Primary touchpoints: `src/background.rs`, `src/app.rs`, `src/tempo.rs`, `src/stretch.rs`, `src/waveform.rs`, and waveform UI in `src/ui.rs`. Proposed modules: `src/analysis_jobs.rs`, `src/model/beat_map.rs`, `src/analysis_worker.rs`, and a versioned worker package under `workers/audio_analysis/`.

Acceptance: editable grids handle steady beats, drifting tempo, pickup notes, half-time ambiguity, and sparse passages; corrected anchors survive reopen and reanalysis; cancellation produces no partially attached source; worker failure leaves playback usable; warp previews and committed sources agree.

**Phase 4 transfers groove and auditions samples against the beat.**

Extract a GrooveTemplate from a selected break using its corrected beat map and transient/accent evidence. Store timing deviations and normalized accent weights separately, with a cycle length and provenance. Permit manual anchor adjustment when a mixed recording has ambiguous transients. Save, rename, and reuse grooves independently of the original audio.

Provide separate Timing and Velocity strength controls, lane selection, and a before/after preview. A 0% amount must preserve notes exactly. Applying a groove bakes the selected transformation into editable event timing in one undo transaction. Programmed drum hits need per-hit timing data; do not reduce the extracted groove to one lane offset. Recorded pad/key performances are transformed only when explicitly selected. Preview and commit must use the Phase 0 timing contract so swing and groove are not applied twice.

Add Audition with beat to the queue, saved cuts, and semantic-search results. Prepare a temporary pitch-preserved loop at project tempo, align its downbeat, and start it on a bar boundary while the current beat continues. Offer original/fit comparison and independent preview level. Use corrected beat maps when available; otherwise ask for a manual bar length in the preview controls. A prepared preview must not alter the source, tempo, arrangement, or undo stack.

Cache audition renders by audio fingerprint, range, beat-map revision, destination tempo, and processing settings. A tempo change invalidates the prepared preview and schedules a replacement at a musical boundary. Unsupported stretch ratios and external-clock jumps should stop or fall back to original-speed audition with a clear status.

Primary touchpoints: `src/model/events.rs`, `src/tempo.rs`, `src/stretch.rs`, `src/audio.rs`, `src/analysis.rs`, `src/ui/step_grid.rs`, and queue UI. Proposed modules: `src/model/groove.rs`, `src/groove.rs`, and `src/audition.rs`.

Acceptance: known synthetic offsets/accent weights are recovered within documented tolerances; 0% is unchanged; lane selection is respected; musical timing matches WAV and MIDI outputs; processed audition starts within one audio callback of its scheduled boundary; repeated audition does not create project edits or block playback.

**Phase 5 builds a local audio index and semantic search.**

Index user-selected folders, saved cuts, and already-downloaded recordings. Use a local database for file identity, content fingerprints, source provenance, timestamps, analysis status, and model version. Index passages rather than only whole tracks. Start with short windows for one-shots/cuts and longer overlapping windows for musical passages; choose final lengths through retrieval evaluation.

Use [CLAP](https://github.com/LAION-AI/CLAP) as the first audio/text embedding candidate. Its repository includes music-oriented checkpoints, but the appropriate checkpoint, runtime, and windowing strategy remain evaluation decisions. Pin the selected model and preprocessing together. Keep indexes for incompatible versions separate and rebuild incrementally rather than comparing embeddings from different spaces.

Support text search and Find similar from a cut, with optional duration, tempo, source, and instrument filters where metadata exists. Combine semantic ranking with the existing rhythm, brightness, energy, and texture features. Add deduplication and adjacent-window merging so results contain useful passages rather than many copies of the same moment. Show a timestamp, audition action, and Assign to pad/Save cut actions. Scores are ranking signals, not confidence percentages.

Use exact vector scoring initially for a bounded local index; adopt an approximate index only when measurements require it. Resume interrupted indexing, detect changed or missing files, cap cache storage, and provide pause/rebuild/remove controls. Searching downloaded audio must not trigger new network downloads. Keep source-specific sampling and decoding off the UI and audio threads.

Primary touchpoints: `src/analysis.rs`, `src/files.rs`, `src/model.rs`, and queue/library UI. Proposed modules: `src/library_index.rs`, `src/search.rs`, and `src/ui/search.rs`.

Acceptance: a fixed set of at least 50 representative queries and human-labeled results demonstrates a useful top-five result for at least 80% of queries that have a match in the test library; mismatches are recorded rather than hidden. Target warm-query p95 below one second at 10,000 passages on the reference machine, including text embedding. Interrupted builds resume, duplicate files do not dominate, changed files invalidate correctly, and indexed search works offline.

**Phase 6 personalizes digging using explicit taste feedback.**

Build an optional local preference profile from Keep/Skip decisions. Treat Later as neutral. Start with a transparent ranking model over the Phase 5 embeddings and available record metadata. Do not infer a dislike from an unheard track or treat every brief preview as negative feedback. Restrict the first implementation to reranking available candidates.

Add a Familiar–Adventurous control that trades similarity against diversity, plus an explanation derived from real score components, such as similarity to kept cuts or matching catalog attributes. Avoid invented descriptions. Persist feedback with source identity and enough history to undo a verdict, rebuild the profile, and reset it. Cold-start behavior uses the existing genre/decade/label filters.

Keep some varied suggestions available so repeated choices do not collapse the queue into near-duplicates. Treat contextual dislikes carefully: a user may skip a good sample because it does not fit the current beat. An optional reason can refine the ranking, but should never be required to move on.

Primary touchpoints: `src/model.rs` queue/verdict/library data, queue functions in `src/app.rs`, `src/catalog.rs`, and `src/search.rs`. Proposed modules: `src/recommendations.rs` and a small local feedback table in the library index.

Acceptance: undo/reset restores the corresponding ranking state; cold-start results remain usable; score explanations match actual inputs; held-out feedback evaluates ranking quality; listening sessions compare recommendations with the current queue before personalization becomes the default. If there is too little feedback to establish improvement, expose it as optional and report that limitation.

**Phase 7 converts isolated audio into editable notes.**

Offer Transcribe to notes for the current selection or an existing stem. Begin with bass and isolated melodic instruments. Evaluate [Basic Pitch](https://github.com/spotify/basic-pitch), whose documented output includes note events and pitch bends and whose recommended input is one instrument at a time. Its published ONNX model makes a Windows runtime evaluation possible; choose Python versus a native runtime after measuring compatibility and packaging cost.

Return source-time note candidates with pitch, onset, duration, strength, and available bends. Convert source seconds through the corrected beat map into project ticks, or use an explicit tempo/alignment choice when no map exists. Display the candidates over the piano roll with source-audio comparison. Let the user set note range, minimum duration, and detection thresholds, then correct notes before applying.

Commit to a new named pattern by default, splitting longer passages into linked patterns/sections when they exceed the existing 16-bar pattern limit. Preview the note result through a selected chromatic instrument and permit choosing another sound without changing the timing. Report unsupported pitches rather than dropping them silently.

Preserve bend data in the model and export where supported. Overlapping notes with different bends cannot be represented faithfully on the current single keys MIDI channel: implement an explicit per-note/channel allocation mode with correct bend-range setup and resets, and retain the existing channel-2 mode for ordinary notes. Offer clearly labeled quantized-pitch export when channel capacity or destination support is insufficient. Rendered audio remains the faithful fallback.

Primary touchpoints: `src/harmony.rs`, `src/model/events.rs`, `src/ui/piano_roll.rs`, `src/audio.rs`, `src/midi_out.rs`, and MIDI export in `src/files.rs`. Proposed modules: `src/transcription.rs` and `src/ui/transcription.rs`.

Acceptance: tests verify seconds-to-ticks conversion, overlap, duration, range handling, and bend resets; a labeled corpus measures onset/pitch/duration quality; imported notes remain fully editable and undoable; changing instruments preserves the phrase; exported MIDI imports correctly and rendered audio preserves supported bends. Publish measured limitations for dense mixes and uncertain notes.

**Phase 8 integrates extraction by description behind an experimental capability.**

Extend the current separation interface to support multiple providers. Keep four-stem Demucs separation available and add an Extract sound tab with a prompt, selected time range, and optional temporal guidance. Audition original, extracted target, and residual before saving. Accepted results become new project sources with their parent source, range, prompt, model/version, and processing settings recorded.

The initial candidate is [SAM Audio](https://github.com/facebookresearch/sam-audio), which supports text and temporal prompting. Its documented setup recommends a CUDA-compatible GPU and requires checkpoint access. Validate actual Windows installation, model availability, memory use, and cancellation behavior before claiming support. Model access and redistribution must be resolved for the selected release; do not assume repository code availability permits bundling every checkpoint.

Begin with short selections and determine supported duration and device limits from measurements. Preserve sample alignment and output lengths. Cache successful results; reject stale results after source/range edits. Worker termination, out-of-memory errors, and unavailable models must leave the original source and project intact. The app should show actionable setup information rather than a control that silently fails.

Compare prompted results against existing Demucs stems and manually selected passages across flute, strings, backing vocals, percussion, and other relevant sounds. Record target isolation and damage to the wanted sound separately. If the early study fails, keep the provider architecture and investigate alternatives; this feature remains pending until the actual extraction experience meets its quality and hardware gates.

Primary touchpoints: `src/separation.rs`, `src/analysis_worker.rs`, `src/analysis_jobs.rs`, `src/app.rs`, and `src/ui/separation.rs` (new).

Acceptance: at least one documented Windows hardware configuration passes setup and repeated extraction; cancel/failure paths publish no partial audio; target/residual alignment is correct; listening tests find a useful extraction on a predeclared set of representative prompts; the experimental label states the measured limits. Only then consider a general release.

**Phase 9 validates the complete workflow and packages each release.**

Use one end-to-end reference session: dig or import a recording; find a passage; correct its beat map; extract a groove; make chops; capture an unarmed performance; create a locked variation; add layers and glide; compare named snapshots; audition another sample in tempo; transcribe a stem; extract a described instrument; arrange; export; and reopen both the project and a DAW session that references a dragged file.

For every feature, verify project save/open, undo/redo, recovery, source identity, cancellation, and applicable playback/export equivalence. Update project migrations and test fixtures in the same change as new persistent data. Use synthetic fixtures for exact timing and routing, and real recorded material for listening evaluation. Share only redistributable fixtures in the repository.

Run focused tests during development, then `cargo test --locked` and `cargo build --release --locked` for each release candidate. Record formatting/lint baseline in Phase 0; fix new issues in changed code and avoid unrelated reformatting. Add worker contract and failure-path tests when the worker is introduced. Tests listed here are planned; this planning pass did not execute them.

Measure UI response and audio callback deadlines while indexing, analyzing, saving recovery, and rendering previews. Target no new allocation/blocking work in newly introduced callback paths, no audio dropouts in a 30-minute reference session at the documented buffer size, and no UI freeze longer than 100 ms for ordinary editing interactions. Measure preprocessing separately from ready-to-play audition latency. These are release targets, not existing performance claims.

Exercise 800×590 and the normal desktop layout, keyboard navigation, MIDI controls, and readable progress/errors. Advanced controls belong in the relevant workflow: capture in Perform/transport, variations in Sequence, maps/grooves/transcription/extraction in Sample, discovery in Queue/library, and A/B comparison in Project. Keep optional model setup out of the initial beat-making path.

Ship a clean Windows build with model capability checks, pinned worker versions, an offline smoke test, cache controls, and updated README/help examples. Verify a fresh machine without Python or a GPU can still use the core app and understand which optional features need setup. Preserve the current user-selected Demucs executable route while adding managed providers.

Acceptance: all twelve feature groups below have passed their applicable gates; experimental extraction is explicitly labeled if still experimental; no feature is called complete merely because its UI or provider stub exists.

| Original proposal | Implementation location | Completion evidence |
| --- | --- | --- |
| Capture what I just played | Phase 1 | Unarmed capture, timing/hold fidelity, undo |
| Groove extraction | Phase 4 | Extract/edit/apply with timing and velocity strength |
| Smarter chopping and warping | Phase 3 | Editable beat/bar map, phrase suggestions, rendered warp |
| Deeper sampler controls | Phases 1–2 | Choke, layers, glide, per-step pitch/filter/start |
| Pattern variations with locks | Phase 1 | Locked content preserved, four auditionable candidates |
| Search samples by sound | Phase 5 | Text/audio queries with passage timestamps |
| Queue that learns taste | Phase 6 | Feedback-driven ranking, diversity, undo/reset |
| Pull an instrument out | Phase 8 | Prompted extraction, auditions, accepted source |
| Sample to editable notes | Phase 7 | Correctable transcription and instrument replay |
| Tempo-matched audition | Phase 4 | In-context aligned preview without project mutation |
| Named A/B snapshots | Phase 1 | Persistent comparisons and undoable restore |
| Drag rendered chops to DAW | Phase 2 | Native drop plus successful DAW-session reopen |

Begin implementation with the Phase 0 compatibility fixtures and event/timing contract, then a narrow vertical slice of retrospective capture from live input through Capture to new pattern, save, reopen, and export. Expand to choke groups, variations, and snapshots only after that path is working. Review scope and estimates at each release gate using measured results rather than committing the entire roadmap to fixed dates now.
