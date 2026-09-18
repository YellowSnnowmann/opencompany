//! What reading a transcript's tail costs, and why it is read from the end
//! (issue #1890 G).
//!
//! `EventLog::read_before` is the primitive every transcript reader opens with:
//! the chat seed, the history route, the GraphQL projection. The filesystem
//! backend used to answer it by streaming from the **head** of the events JSONL
//! and parsing every line to keep the last few, so one page cost O(total
//! company events) however few pages were asked for.
//!
//! That cost was always there and was paid on each channel switch. Scoping the
//! chat seed to a thread (#1890 A) turned a rebind from per-channel into
//! per-*thread*, so on a company with a long history it became the dominant
//! cost of answering a message — which is what this bench measures, and what
//! reading backwards fixes.
//!
//! Two shapes, over journals of growing length:
//!
//! * `forward` — the previous implementation, reproduced here so the comparison
//!   is measured rather than argued. Streams from the head, parses every line,
//!   keeps a sliding tail of `limit`.
//! * `backward` — the current one. Seeks to the end and reads fixed chunks
//!   until the page is full, parsing only the lines it returns.
//!
//! # Reading these numbers honestly
//!
//! * **The shape is the finding, not the absolute.** `forward` is linear in the
//!   journal's length and `backward` is flat in it; the crossover is immediate
//!   and the gap widens without bound. A local SSD's constants do not predict a
//!   network volume's, but the slopes do not depend on the medium.
//! * **The page size is fixed at `EVENT_PAGE`-scale** (512), because that is
//!   what the seed asks for. A larger page moves `backward` up by a constant
//!   and leaves `forward` where it is.
//! * **The OS page cache is warm** — each iteration re-reads a file just
//!   written. That flatters both, and flatters `forward` more, since it is the
//!   one doing the extra reading. Cold, the gap is wider.
//!
//! # Measured baseline (macOS 15 / APFS / local SSD, `cargo bench`)
//!
//! One 512-record page, off journals of growing length:
//!
//! | journal | `forward` | `backward` |
//! |---|---|---|
//! | 1,000 events | 759.9µs | 397.5µs |
//! | 10,000 events | 7.25ms | 394.0µs |
//! | 100,000 events | 72.80ms | 399.8µs |
//!
//! `forward` is exactly linear — ten times the journal, ten times the time —
//! and `backward` is flat to within measurement noise, because it reads the
//! page and nothing else. At 100k events, which is an ordinary year for a busy
//! company, that is **182× less work for the same answer**, and the ratio keeps
//! growing because only one of the two curves has a slope.
//!
//! `backward`'s ~397µs is not overhead: it is the cost of reading and parsing
//! the 512 records the caller asked for, which is the irreducible work. The
//! whole of `forward`'s excess was spent parsing records it then discarded.
//!
//! Run with `cargo bench --bench journal_read_before`. Not wired into CI, for
//! the same reason `journal_append` is not: it measures the host's storage.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

/// A line's worth of realistic journal record — an operator message with the
/// envelope every stored event carries.
fn record(seq: u64) -> String {
    format!(
        r#"{{"seq":{seq},"company":"acme","at_millis":1750000000000,"event":{{"kind":"OperatorMessage","text":"a message of roughly the length an operator actually types when they are asking for something","chat":"growth","parent":null}}}}"#
    )
}

fn write_journal(path: &Path, lines: usize) {
    let mut file = File::create(path).expect("create");
    for seq in 0..lines {
        writeln!(file, "{}", record(seq as u64)).expect("write");
    }
    file.sync_all().expect("sync");
}

/// The implementation this issue replaced: head-first, parsing everything.
fn forward_tail(path: &Path, limit: usize) -> usize {
    let file = File::open(path).expect("open");
    let mut tail: std::collections::VecDeque<serde_json::Value> = std::collections::VecDeque::new();
    for line in BufReader::new(file).lines() {
        let line = line.expect("line");
        if line.trim().is_empty() {
            continue;
        }
        let event: serde_json::Value = serde_json::from_str(&line).expect("parse");
        if tail.len() == limit {
            tail.pop_front();
        }
        tail.push_back(event);
    }
    tail.len()
}

/// The current one: from the end, parsing only what is returned.
fn backward_tail(path: &Path, limit: usize) -> usize {
    const CHUNK: u64 = 64 * 1024;
    let mut file = File::open(path).expect("open");
    let mut pos = file.metadata().expect("meta").len();
    let mut carry: Vec<u8> = Vec::new();
    let mut kept = 0usize;
    while pos > 0 && kept < limit {
        let take = CHUNK.min(pos);
        pos -= take;
        file.seek(SeekFrom::Start(pos)).expect("seek");
        let mut chunk = vec![0u8; take as usize];
        file.read_exact(&mut chunk).expect("read");
        chunk.extend_from_slice(&carry);
        let mut segments: Vec<&[u8]> = chunk.split(|byte| *byte == b'\n').collect();
        carry = if pos == 0 {
            Vec::new()
        } else {
            segments.remove(0).to_vec()
        };
        for segment in segments.iter().rev() {
            let line = match std::str::from_utf8(segment) {
                Ok(line) => line.trim(),
                Err(_) => continue,
            };
            if line.is_empty() {
                continue;
            }
            let _event: serde_json::Value = serde_json::from_str(line).expect("parse");
            kept += 1;
            if kept == limit {
                break;
            }
        }
    }
    kept
}

fn bench(c: &mut Criterion) {
    // One page, as the chat seed asks for it.
    const LIMIT: usize = 512;
    let dir = tempfile::Builder::new()
        .prefix("read-before-bench-")
        .tempdir()
        .expect("tempdir");

    let mut group = c.benchmark_group("read_before/tail_page");
    // A young company, a busy one, and one a year in — the range over which the
    // two shapes separate.
    for lines in [1_000usize, 10_000, 100_000] {
        let path = dir.path().join(format!("events-{lines}.jsonl"));
        write_journal(&path, lines);

        group.bench_with_input(BenchmarkId::new("forward", lines), &path, |b, path| {
            b.iter(|| forward_tail(path, LIMIT))
        });
        group.bench_with_input(BenchmarkId::new("backward", lines), &path, |b, path| {
            b.iter(|| backward_tail(path, LIMIT))
        });
    }
    group.finish();
}

criterion_group!(benches, bench);
criterion_main!(benches);
