use std::{
    fs,
    io::{self, Cursor, Read},
    os::unix::fs::PermissionsExt,
    sync::{Arc, Barrier},
    thread,
};

use hegel::{TestCase, generators as gs};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use super::{Artifact, Bytes, Error, Quotas};

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
