use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Command, Stdio},
};

fn write_clicks(path: &Path, seconds: u32) {
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 8_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    for frame in 0..seconds * 8_000 {
        let value = if frame >= 1_360 && (frame - 1_360) % 4_000 < 40 {
            21_000_i16
        } else {
            0
        };
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();
}

fn request(path: &Path, seconds: usize) -> Value {
    json!({
        "version": 1, "type": "run", "job_id": 41, "project_revision": 8, "source_id": 3,
        "source_fingerprint": "fixture", "start_frame": 16_000, "end_frame": 16_000 + seconds * 8_000,
        "sample_rate": 8_000, "operation": "beats", "model_id": "lost-samples-pulse",
        "model_version": "1", "settings": {"bpm_hint": 120}, "input_path": path,
        "output_dir": path.parent().unwrap(), "timeout_ms": 5_000
    })
}

fn run(request: Value) -> Vec<Value> {
    let mut worker = Command::new(env!("CARGO_BIN_EXE_lost-samples-desktop"))
        .arg("--audio-analysis-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(worker.stdin.take().unwrap(), "{request}").unwrap();
    let result = worker.wait_with_output().unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn subprocess_returns_versioned_beats_relative_to_the_file_range() {
    let directory = tempfile::tempdir().unwrap();
    let audio = directory.path().join("clicks.wav");
    write_clicks(&audio, 8);
    let messages = run(request(&audio, 8));
    assert_eq!(messages[0]["type"], "progress");
    let result = messages.last().unwrap();
    assert_eq!(result["type"], "result");
    assert_eq!(result["job_id"], 41);
    assert_eq!(result["project_revision"], 8);
    assert_eq!(result["start_frame"], 16_000);
    let beats = result["beats"].as_array().unwrap();
    assert!(beats.len().abs_diff(16) <= 1);
    assert!((beats[0]["seconds"].as_f64().unwrap() - 0.17).abs() < 0.025);
    assert_eq!(beats[0]["downbeat"], true);
    assert_eq!(beats[4]["downbeat"], true);
}

#[test]
fn unsupported_protocol_returns_a_structured_failure() {
    let directory = tempfile::tempdir().unwrap();
    let mut message = request(&directory.path().join("absent.wav"), 8);
    message["version"] = json!(99);
    let result = run(message);
    assert_eq!(result[0]["type"], "failure");
    assert_eq!(result[0]["code"], "invalid_request");
}

#[test]
fn subprocess_cancels_before_producing_a_result() {
    let directory = tempfile::tempdir().unwrap();
    let audio = directory.path().join("long.wav");
    write_clicks(&audio, 120);
    let mut worker = Command::new(env!("CARGO_BIN_EXE_lost-samples-desktop"))
        .arg("--audio-analysis-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = worker.stdin.take().unwrap();
    writeln!(input, "{}", request(&audio, 120)).unwrap();
    let mut output = BufReader::new(worker.stdout.take().unwrap()).lines();
    let first: Value = serde_json::from_str(&output.next().unwrap().unwrap()).unwrap();
    assert_eq!(first["type"], "progress");
    writeln!(
        input,
        "{}",
        json!({"version": 1, "type": "cancel", "job_id": 41})
    )
    .unwrap();
    drop(input);
    let terminal: Value = serde_json::from_str(&output.last().unwrap().unwrap()).unwrap();
    assert_eq!(terminal["type"], "failure");
    assert!(terminal["message"].as_str().unwrap().contains("cancel"));
    assert!(worker.wait().unwrap().success());
}
