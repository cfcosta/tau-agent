use std::{
    fs,
    io::{self, Cursor, Read},
    os::unix::fs::PermissionsExt,
    sync::{Arc, Barrier},
    thread,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use hegel::{TestCase, generators as gs};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::{Artifact, Bytes, Encoding, Error, MAX_RANGE_BYTES, Quotas};

fn objects(storage: &tempfile::TempDir) -> Vec<String> {
    fs::read_dir(storage.path().join("objects"))
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().into_string().unwrap();
            name.ends_with(".blob").then_some(name)
        })
        .collect()
}

fn storage(quotas: Quotas) -> (tempfile::TempDir, Bytes) {
    let directory = tempfile::tempdir().unwrap();
    let bytes = Bytes::new(directory.path(), quotas).unwrap();
    (directory, bytes)
}

fn published(input: &[u8]) -> (tempfile::TempDir, Bytes, Artifact) {
    let (directory, bytes) = storage(Quotas::default());
    let artifact = bytes
        .publish_reader(Cursor::new(input.to_vec()), &CancellationToken::new())
        .unwrap();
    (directory, bytes, artifact)
}

#[test]
fn base64_ranges_keep_binary_bytes_and_report_artifact_end() {
    let input = [0, 0xff, b'a', 0, 0x80];
    let (_directory, bytes, artifact) = published(&input);
    let cancel = CancellationToken::new();
    let first = bytes
        .read_range(&artifact, 1, 2, Encoding::Base64, &cancel)
        .unwrap();
    assert_eq!(first.id, artifact.id());
    assert_eq!(first.offset, 1);
    assert_eq!(first.size_bytes, input.len() as u64);
    assert_eq!(STANDARD.decode(first.data).unwrap(), input[1..3]);
    assert_eq!(first.next_offset, Some(3));
    assert!(!first.eof && !first.complete);
    let last = bytes
        .read_range(&artifact, 3, 2, Encoding::Base64, &cancel)
        .unwrap();
    assert_eq!(STANDARD.decode(last.data).unwrap(), input[3..]);
    assert_eq!(last.next_offset, None);
    assert!(last.eof);
    assert!(!last.complete);
    let empty = bytes
        .read_range(&artifact, 5, 1, Encoding::Base64, &cancel)
        .unwrap();
    assert_eq!(empty.data, "");
    assert_eq!(empty.next_offset, None);
    assert!(empty.eof);
    assert!(!empty.complete);
}

#[test]
fn utf8_pages_end_on_codepoint_boundaries_and_never_stall() {
    let input = "Aé🦀Z";
    let (_directory, bytes, artifact) = published(input.as_bytes());
    let cancel = CancellationToken::new();
    let first = bytes
        .read_range(&artifact, 0, 2, Encoding::Utf8, &cancel)
        .unwrap();
    assert_eq!(first.data, "A");
    assert_eq!(first.next_offset, Some(1));
    assert!(!first.complete);
    assert!(matches!(
        bytes.read_range(&artifact, 2, 3, Encoding::Utf8, &cancel),
        Err(Error::Utf8Start)
    ));
    assert!(matches!(
        bytes.read_range(&artifact, 1, 1, Encoding::Utf8, &cancel),
        Err(Error::Utf8LimitTooSmall)
    ));
    let second = bytes
        .read_range(&artifact, 1, 5, Encoding::Utf8, &cancel)
        .unwrap();
    assert_eq!(second.data, "é");
    assert_eq!(second.next_offset, Some(3));
    let last = bytes
        .read_range(&artifact, 3, 5, Encoding::Utf8, &cancel)
        .unwrap();
    assert_eq!(last.data, "🦀Z");
    assert!(last.eof);
    assert!(!last.complete);
}

#[test]
fn invalid_bytes_limits_offsets_and_missing_files_fail_explicitly() {
    let (directory, bytes, artifact) = published(&[b'a', 0xff, b'z']);
    let cancel = CancellationToken::new();
    assert!(matches!(
        bytes.read_range(&artifact, 0, 3, Encoding::Utf8, &cancel),
        Err(Error::InvalidUtf8)
    ));
    let (_truncated_dir, truncated_bytes, truncated) =
        published(&[b'a', 0xf0, 0x9f]);
    assert!(matches!(
        truncated_bytes.read_range(&truncated, 0, 3, Encoding::Utf8, &cancel),
        Err(Error::InvalidUtf8)
    ));
    assert!(matches!(
        bytes.read_range(&artifact, 0, 0, Encoding::Base64, &cancel),
        Err(Error::InvalidLimit)
    ));
    assert!(matches!(
        bytes.read_range(
            &artifact,
            0,
            MAX_RANGE_BYTES + 1,
            Encoding::Base64,
            &cancel
        ),
        Err(Error::InvalidLimit)
    ));
    assert!(matches!(
        bytes.read_range(&artifact, 4, 1, Encoding::Base64, &cancel),
        Err(Error::OffsetOutOfRange)
    ));
    let stopped = CancellationToken::new();
    stopped.cancel();
    assert!(matches!(
        bytes.read_range(&artifact, 0, 1, Encoding::Base64, &stopped),
        Err(Error::Cancelled)
    ));
    let path = directory
        .path()
        .join("objects")
        .join(format!("{}.blob", artifact.id()));
    fs::remove_file(&path).unwrap();
    assert!(matches!(
        bytes.read_range(&artifact, 0, 1, Encoding::Base64, &cancel),
        Err(Error::MissingArtifact)
    ));
    fs::write(path, [0; 4]).unwrap();
    assert!(matches!(
        bytes.read_range(&artifact, 0, 1, Encoding::Base64, &cancel),
        Err(Error::InvalidArtifact)
    ));
}

#[test]
fn maximum_range_reads_exactly_65536_bytes() {
    let input = vec![0x80; MAX_RANGE_BYTES as usize + 1];
    let (_directory, bytes, artifact) = published(&input);
    let page = bytes
        .read_range(
            &artifact,
            0,
            MAX_RANGE_BYTES,
            Encoding::Base64,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(
        STANDARD.decode(page.data).unwrap(),
        input[..MAX_RANGE_BYTES as usize]
    );
    assert_eq!(page.next_offset, Some(MAX_RANGE_BYTES as u64));
    assert!(!page.complete);
}

// Inventory: arbitrary bytes/offsets/limits compare with an independent slice
// and base64 oracle; valid Unicode pages concatenate to the source, including
// limits inside a codepoint. Offsets and limits are valid by construction, so
// neither property rejects cases. Small vectors shrink to empty/minimal pages.
// CI uses the workspace hegel.toml ci profile and its deterministic seed;
// no local case override or persistent CI database is needed.
#[hegel::test]
fn base64_ranges_match_independent_slices(tc: TestCase) {
    let input: Vec<u8> = tc.draw(gs::vecs(gs::integers::<u8>()).max_size(256));
    let offset_seed: u16 = tc.draw(gs::integers());
    let limit: u32 =
        tc.draw(gs::integers().min_value(1).max_value(MAX_RANGE_BYTES));
    let offset = usize::from(offset_seed) % (input.len() + 1);
    let expected_end = (offset + limit as usize).min(input.len());
    let (_directory, bytes, artifact) = published(&input);
    let range = bytes
        .read_range(
            &artifact,
            offset as u64,
            limit,
            Encoding::Base64,
            &CancellationToken::new(),
        )
        .unwrap();
    assert_eq!(
        STANDARD.decode(range.data).unwrap(),
        input[offset..expected_end]
    );
    assert_eq!(range.offset, offset as u64);
    assert_eq!(range.size_bytes, input.len() as u64);
    assert_eq!(
        range.next_offset,
        (expected_end < input.len()).then_some(expected_end as u64)
    );
    assert_eq!(range.eof, expected_end == input.len());
    assert_eq!(range.complete, offset == 0 && expected_end == input.len());
}

#[hegel::test]
fn unicode_pages_concatenate_without_losing_codepoints(tc: TestCase) {
    let source: String = tc.draw(gs::text().max_size(96));
    let limit: u32 = tc.draw(gs::integers().min_value(4).max_value(25));
    let (_directory, bytes, artifact) = published(source.as_bytes());
    let mut offset = 0;
    let mut joined = String::new();
    loop {
        let page = bytes
            .read_range(
                &artifact,
                offset,
                limit,
                Encoding::Utf8,
                &CancellationToken::new(),
            )
            .unwrap();
        joined.push_str(&page.data);
        if page.eof {
            assert_eq!(page.next_offset, None);
            break;
        }
        let next = page.next_offset.unwrap();
        assert!(next > offset);
        offset = next;
    }
    assert_eq!(joined, source);
}

struct ChunkedReader {
    bytes: Cursor<Vec<u8>>,
    splits: Vec<u16>,
    next_split: usize,
}

impl Read for ChunkedReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let split =
            self.splits[self.next_split % self.splits.len()] as usize + 1;
        self.next_split += 1;
        let count = output.len().min(split);
        self.bytes.read(&mut output[..count])
    }
}

// Property inventory: bounded raw bytes plus UTF-8 bytes survive publication
// and private read exactly; an independent one-shot SHA-256 is the digest
// oracle, while input length is the size oracle. Chunk boundaries also probe
// streaming reads that split multibyte Unicode sequences.
// Generator plan: generate all inputs valid by construction (0..4096 bytes,
// 0..64 Unicode characters, 1..12 split widths); small vectors and widths
// shrink toward an empty/one-chunk counterexample, with no rejected cases.
// CI: the workspace hegel.toml selects Hegel's deterministic ci profile,
// disables its example database, and suppresses TooSlow. No per-test count.
#[hegel::test]
fn bounded_bytes_round_trip(tc: TestCase) {
    let mut input: Vec<u8> =
        tc.draw(gs::vecs(gs::integers::<u8>()).max_size(4096));
    let unicode: String = tc.draw(gs::text().max_size(64));
    let splits: Vec<u16> = tc.draw(
        gs::vecs(gs::integers::<u16>().max_value(11))
            .min_size(1)
            .max_size(8),
    );
    input.extend_from_slice(unicode.as_bytes());

    let (_directory, bytes) = storage(Quotas {
        max_artifact_bytes: 8192,
        total_bytes: 8192,
    });
    let reader = ChunkedReader {
        bytes: Cursor::new(input.clone()),
        splits,
        next_split: 0,
    };
    let artifact = bytes
        .publish_reader(reader, &CancellationToken::new())
        .unwrap();
    let mut actual = Vec::new();
    bytes
        .open_object(&artifact)
        .unwrap()
        .read_to_end(&mut actual)
        .unwrap();

    assert_eq!(actual, input);
    assert_eq!(artifact.size_bytes(), input.len() as u64);
    let expected_digest: String = Sha256::digest(&input)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(artifact.digest(), expected_digest);
    assert_eq!(artifact.id().len(), 36);
}

#[test]
fn failed_and_cancelled_streams_leave_no_object() {
    struct FailingReader(bool);
    impl Read for FailingReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            if self.0 {
                Err(io::Error::other("reader failed"))
            } else {
                self.0 = true;
                output[0] = b'a';
                Ok(1)
            }
        }
    }

    let (directory, bytes) = storage(Quotas {
        max_artifact_bytes: 2,
        total_bytes: 10,
    });
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        bytes.publish_reader(Cursor::new(b"a"), &cancelled),
        Err(Error::Cancelled)
    ));
    assert!(matches!(
        bytes.publish_reader(FailingReader(false), &CancellationToken::new()),
        Err(Error::Io(_))
    ));
    assert!(matches!(
        bytes.publish_reader(Cursor::new(b"abc"), &CancellationToken::new()),
        Err(Error::ArtifactTooLarge)
    ));
    assert!(objects(&directory).is_empty());
    assert_eq!(
        fs::read_dir(directory.path().join("objects"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn cancellation_after_a_read_prevents_publication() {
    struct CancellingReader(CancellationToken);
    impl Read for CancellingReader {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            output[0] = 1;
            self.0.cancel();
            Ok(1)
        }
    }
    let (directory, bytes) = storage(Quotas::default());
    let cancellation = CancellationToken::new();
    assert!(matches!(
        bytes.publish_reader(
            CancellingReader(cancellation.clone()),
            &cancellation
        ),
        Err(Error::Cancelled)
    ));
    assert!(objects(&directory).is_empty());
}

#[test]
fn total_quota_counts_published_bytes_after_restart() {
    let (directory, bytes) = storage(Quotas {
        max_artifact_bytes: 6,
        total_bytes: 10,
    });
    let first = bytes
        .publish_reader(Cursor::new(b"123456"), &CancellationToken::new())
        .unwrap();
    let reopened = Bytes::new(
        directory.path(),
        Quotas {
            max_artifact_bytes: 6,
            total_bytes: 10,
        },
    )
    .unwrap();
    assert_eq!(
        reopened
            .open_object(&first)
            .unwrap()
            .metadata()
            .unwrap()
            .len(),
        6
    );
    assert!(matches!(
        reopened
            .publish_reader(Cursor::new(b"12345"), &CancellationToken::new()),
        Err(Error::TotalQuotaExceeded)
    ));
    assert_eq!(objects(&directory).len(), 1);
    let second = reopened
        .publish_reader(Cursor::new(b"1234"), &CancellationToken::new())
        .unwrap();
    assert_ne!(first.id(), second.id());
    assert_eq!(objects(&directory).len(), 2);
}

#[test]
fn concurrent_handles_do_not_over_admit() {
    let (directory, first) = storage(Quotas {
        max_artifact_bytes: 6,
        total_bytes: 10,
    });
    let second = Bytes::new(
        directory.path(),
        Quotas {
            max_artifact_bytes: 6,
            total_bytes: 10,
        },
    )
    .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let workers = [first, second].map(|bytes| {
        let barrier = barrier.clone();
        thread::spawn(move || {
            barrier.wait();
            bytes.publish_reader(
                Cursor::new(b"123456"),
                &CancellationToken::new(),
            )
        })
    });
    barrier.wait();
    let results = workers.map(|worker| worker.join().unwrap());
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(Error::TotalQuotaExceeded)))
            .count(),
        1
    );
    assert_eq!(objects(&directory).len(), 1);
}

#[test]
fn published_files_are_private_and_read_only() {
    let (directory, bytes) = storage(Quotas::default());
    let artifact = bytes
        .publish_reader(Cursor::new(b"private"), &CancellationToken::new())
        .unwrap();
    let path = directory
        .path()
        .join("objects")
        .join(format!("{}.blob", artifact.id()));
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o400
    );
    assert_eq!(
        fs::metadata(directory.path().join("objects"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn deserialization_rejects_paths_and_invalid_digests() {
    let (directory, bytes) = storage(Quotas::default());
    let artifact = bytes
        .publish_reader(Cursor::new(b"a"), &CancellationToken::new())
        .unwrap();
    let mut value = serde_json::to_value(&artifact).unwrap();
    assert_eq!(
        serde_json::from_value::<Artifact>(value.clone()).unwrap(),
        artifact
    );
    value["id"] = serde_json::json!("../victim");
    assert!(serde_json::from_value::<Artifact>(value.clone()).is_err());
    value["id"] = serde_json::json!(artifact.id().to_uppercase());
    assert!(serde_json::from_value::<Artifact>(value.clone()).is_err());
    value["id"] = serde_json::json!(artifact.id());
    value["digest"] = serde_json::json!(artifact.digest().to_uppercase());
    assert!(serde_json::from_value::<Artifact>(value.clone()).is_err());
    value["digest"] = serde_json::json!("../bad");
    assert!(serde_json::from_value::<Artifact>(value).is_err());
    assert_eq!(objects(&directory).len(), 1);
}
