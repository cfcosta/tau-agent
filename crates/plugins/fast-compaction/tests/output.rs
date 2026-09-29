//! Output pruning's pure parts, over generated outputs and histories:
//! the gate, the lines and chunks, the rendering, the decisions, the
//! requests, and the history's segments.

use std::collections::{BTreeMap, BTreeSet};

use hegel::{TestCase, generators as gs, generators::Generator as _};
use tau_ai::message::{InputBlock, TextContent};
use tau_fast_compaction::{
    OutputPruning,
    history::{CallRecord, Field, Record, ResultRecord, split_history},
    output::{self, HEADER, MAX_CHUNKS, MAX_LINE_CHARS, SPILL},
    plan::{REQUEST_OVERHEAD_TOKENS, Tally},
    state::{Role, estimate_state_tokens, estimate_tokens},
};
use tau_jev::{Answer, JevError, fake::FakeJev};
use tau_testing::block_on;

fn text_block(text: &str) -> InputBlock {
    InputBlock::Text(TextContent {
        text: text.to_owned(),
        text_signature: None,
    })
}

/// Text of exactly a drawn length up to `max_chars`, from `alphabet`.
#[hegel::composite]
fn sized(tc: &TestCase, alphabet: &'static str, max_chars: usize) -> String {
    let chars = tc.draw(gs::integers::<usize>().max_value(max_chars));
    tc.draw(
        gs::text()
            .alphabet(alphabet)
            .min_size(chars)
            .max_size(chars),
    )
}

/// Characters an output line is made of: never `[` or a newline, so a
/// line never reads as a marker.
const LINE: &str = "abc xyz0189.:-_/é日🦀";

/// An output: up to 300 lines, most short, some over
/// [`MAX_LINE_CHARS`] characters.
#[hegel::composite]
fn output_text(tc: &TestCase) -> String {
    let count = tc.draw(gs::integers::<usize>().min_value(1).max_value(300));
    (0..count)
        .map(|_| {
            if tc.draw(gs::integers::<u8>().max_value(40)) == 0 {
                tc.event("a long line");
                let chars = tc.draw(
                    gs::integers::<usize>()
                        .min_value(MAX_LINE_CHARS + 1)
                        .max_value(3 * MAX_LINE_CHARS + 7),
                );
                "ab 1".chars().cycle().take(chars).collect()
            } else {
                tc.draw(sized(LINE, 60))
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// The gate lets through only a successful `bash` result of text,
/// without a NUL byte, estimated over the threshold, and then takes the
/// result's text as the whole output. Everything else goes untouched,
/// with no Jev call.
#[hegel::test(test_cases = 200)]
fn the_gate_passes_only_large_successful_text(tc: TestCase) {
    let tool = tc.draw(gs::sampled_from(vec!["bash", "read"]));
    let is_error = tc.draw(gs::booleans());
    let mut text = tc.draw(output_text());
    if tc.draw(gs::booleans()) {
        text.push('\0');
    }
    let min = tc.draw(gs::integers::<usize>().max_value(3000));
    let mut content = vec![text_block(&text)];
    let image = tc.draw(gs::integers::<u8>().max_value(5)) == 0;
    if image {
        content.push(InputBlock::Image(
            tc.draw(tau_testing::generators::image_content()),
        ));
    }
    let expected = tool == "bash"
        && !is_error
        && !image
        && !text.contains('\0')
        && estimate_tokens(&text) > min;
    tc.event(if expected { "passes" } else { "untouched" });
    let gated = output::gate(tool, is_error, &content, min);
    assert_eq!(gated.is_some(), expected);
    if let Some(gated) = gated {
        assert_eq!(gated.full, text);
        assert_eq!(gated.seen, text);
        assert_eq!(gated.spill, None);
    }
}

/// When `bash` spilled the output to its file, the gate reads the whole
/// output from it; a file of invalid UTF-8 leaves the result untouched;
/// a path `bash` would not have written is not read.
#[test]
fn the_gate_reads_what_bash_spilled() {
    let dir = std::env::temp_dir()
        .join(format!("tau-fast-compaction-gate-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let full = (0..3000)
        .map(|n| format!("line {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let spill = dir.join("tau-bash-00ff.log");
    std::fs::write(&spill, &full).unwrap();
    let seen = format!("line 2999{SPILL}{}", spill.display());
    let gated = output::gate("bash", false, &[text_block(&seen)], 100).unwrap();
    assert_eq!(gated.full, full);
    assert_eq!(gated.seen, seen);
    assert_eq!(gated.spill.as_deref(), Some(spill.as_path()));

    std::fs::write(&spill, [b'a', 0xff, b'\n'].repeat(4000)).unwrap();
    assert_eq!(output::gate("bash", false, &[text_block(&seen)], 100), None);

    let other = dir.join("notes.log");
    std::fs::write(&other, &full).unwrap();
    let seen = format!("{}{SPILL}{}", "word ".repeat(200), other.display());
    let gated = output::gate("bash", false, &[text_block(&seen)], 100).unwrap();
    assert_eq!(gated.full, seen);
    assert_eq!(gated.spill, None);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// What a rendered body says, line by line: a kept line, or a count of
/// omitted ones.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Said {
    Line(String),
    Omitted(usize),
}

/// The lines of an output make it up exactly, none over
/// [`MAX_LINE_CHARS`]; its chunks cover them in order, at most
/// [`MAX_CHUNKS`]. Rendering any choice of chunks keeps every kept line
/// verbatim and in order, a long line's pieces together when kept
/// together, and puts one marker for each run of dropped lines, counting
/// exactly them; the header comes first, and the footer names the
/// archive.
#[hegel::test(test_cases = 300)]
fn rendering_keeps_lines_verbatim_and_counts_what_it_omits(tc: TestCase) {
    let text = tc.draw(output_text());
    let lines = output::lines(&text);
    let mut rebuilt = String::new();
    for (index, line) in lines.iter().enumerate() {
        assert!(line.text.chars().count() <= MAX_LINE_CHARS);
        rebuilt.push_str(line.text);
        if line.ends && index + 1 < lines.len() {
            rebuilt.push('\n');
        }
    }
    assert_eq!(rebuilt, text);

    let chunk_lines = tc.draw(gs::integers::<usize>().max_value(30));
    let chunks = output::chunks(lines.len(), chunk_lines);
    assert!(!chunks.is_empty() && chunks.len() <= MAX_CHUNKS);
    assert_eq!(chunks[0].start, 0);
    assert_eq!(chunks.last().unwrap().end, lines.len());
    for pair in chunks.windows(2) {
        assert_eq!(pair[0].end, pair[1].start);
    }

    let keep: Vec<bool> =
        chunks.iter().map(|_| tc.draw(gs::booleans())).collect();
    let archive = "/archive/of/it.txt";
    let rendered = output::render(&lines, &chunks, &keep, archive);
    let body = rendered
        .strip_prefix(&format!("{HEADER}\n"))
        .unwrap()
        .strip_suffix(&format!(
            "\n\n[full output: {archive} (read or grep it if needed)]"
        ))
        .unwrap();

    let mut expected: Vec<Said> = Vec::new();
    let mut open = false;
    for (range, kept) in chunks.iter().zip(&keep) {
        for line in &lines[range.clone()] {
            if *kept {
                match expected.last_mut() {
                    Some(Said::Line(text)) if open => text.push_str(line.text),
                    _ => expected.push(Said::Line(line.text.to_owned())),
                }
                open = !line.ends;
            } else {
                match expected.last_mut() {
                    Some(Said::Omitted(count)) => *count += 1,
                    _ => expected.push(Said::Omitted(1)),
                }
                open = false;
            }
        }
    }
    let said: Vec<Said> = body
        .split('\n')
        .map(|line| {
            match line
                .strip_prefix('[')
                .and_then(|rest| rest.strip_suffix(" lines omitted]"))
            {
                Some(count) => Said::Omitted(count.parse().unwrap()),
                None => Said::Line(line.to_owned()),
            }
        })
        .collect();
    if said.iter().any(|said| matches!(said, Said::Omitted(_))) {
        tc.event("lines omitted");
    }
    assert_eq!(said, expected);
}

/// A chunk goes only when it is neither the first nor the last, every
/// segment answered about it, and every answer was under the threshold;
/// an unanswered segment (a failed request, or one past the allowance)
/// keeps it.
#[hegel::test(test_cases = 300)]
fn only_a_chunk_every_segment_scored_under_the_threshold_goes(tc: TestCase) {
    let chunks = tc.draw(gs::integers::<usize>().min_value(1).max_value(40));
    let segments = tc.draw(gs::integers::<usize>().min_value(1).max_value(4));
    let threshold = tc.draw(gs::sampled_from(vec![0.0, 0.05, 0.1, 0.5, 0.9]));
    let mut tally = Tally::new(chunks, segments);
    let mut answers = vec![vec![None; segments]; chunks];
    for (chunk, row) in answers.iter_mut().enumerate() {
        for (segment, slot) in row.iter_mut().enumerate() {
            let answer: Option<f64> = tc.draw(gs::sampled_from(vec![
                None,
                Some(0.0),
                Some(0.05),
                Some(0.1),
                Some(0.2),
                Some(0.49),
                Some(0.5),
                Some(1.0),
            ]));
            if let Some(noul) = answer {
                tally.record(chunk, segment, vec![noul]);
            }
            *slot = answer;
        }
    }
    let keep = output::keep(chunks, &tally, threshold);
    for (chunk, row) in answers.iter().enumerate() {
        let disposable = chunk != 0
            && chunk + 1 != chunks
            && row
                .iter()
                .all(|answer| answer.is_some_and(|noul| noul < threshold));
        if disposable {
            tc.event("a chunk goes");
        }
        assert_eq!(keep[chunk], !disposable, "chunk {chunk}: {row:?}");
    }
}

/// A record of a drawn history, message `i`.
#[hegel::composite]
fn record(tc: &TestCase, i: usize) -> Record {
    const TEXT: &str = "ab cd\n0189.,\"{é🦀";
    let big = if tc.draw(gs::integers::<u8>().max_value(4)) == 0 {
        6000
    } else {
        200
    };
    let calls = tc.draw(gs::integers::<usize>().max_value(2));
    let results = tc.draw(gs::integers::<usize>().max_value(2));
    Record {
        i,
        role: if tc.draw(gs::booleans()) {
            Role::User
        } else {
            Role::Assistant
        },
        text: tc.draw(sized(TEXT, big)),
        tool_calls: (0..calls)
            .map(|n| CallRecord {
                id: format!("t{i}{n}"),
                tool: "bash".into(),
                input: tc.draw(sized(TEXT, big)),
                result: tc
                    .draw(gs::booleans())
                    .then(|| tc.draw(sized(TEXT, 40))),
            })
            .collect(),
        tool_results: (0..results)
            .map(|n| ResultRecord {
                id: format!("t{i}{n}"),
                is_error: tc.draw(gs::booleans()),
                result: tc.draw(sized(TEXT, big)),
            })
            .collect(),
        part: None,
    }
}

/// A drawn history of up to 8 records.
#[hegel::composite]
fn records(tc: &TestCase) -> Vec<Record> {
    let count = tc.draw(gs::integers::<usize>().max_value(8));
    (0..count)
        .map(|i| tc.draw(record(i).print_as_debug()))
        .collect()
}

/// The fields of a record, as continuations name them, with the call or
/// result they belong to and their text.
fn fields(record: &Record) -> Vec<(Field, String, String)> {
    let mut fields = Vec::new();
    if !record.text.is_empty() {
        fields.push((Field::Text, String::new(), record.text.clone()));
    }
    for call in &record.tool_calls {
        fields.push((Field::CallInput, call.id.clone(), call.input.clone()));
        if let Some(result) = &call.result {
            fields.push((Field::CallResult, call.id.clone(), result.clone()));
        }
    }
    for result in &record.tool_results {
        fields.push((
            Field::ResultText,
            result.id.clone(),
            result.result.clone(),
        ));
    }
    fields
}

/// What a continuation holds: its field, the call or result it belongs
/// to, and its piece of the field.
fn piece(fragment: &Record) -> (Field, String, String) {
    let field = fragment.part.unwrap().field;
    match field {
        Field::Text => (field, String::new(), fragment.text.clone()),
        Field::CallInput => {
            let call = &fragment.tool_calls[0];
            (field, call.id.clone(), call.input.clone())
        }
        Field::CallResult => {
            let call = &fragment.tool_calls[0];
            (field, call.id.clone(), call.result.clone().unwrap())
        }
        Field::ResultText => {
            let result = &fragment.tool_results[0];
            (field, result.id.clone(), result.result.clone())
        }
    }
}

/// Splitting a history loses nothing and truncates nothing: every
/// record lands in exactly one place, in order, whole when it fits
/// alone, otherwise as continuations of each of its fields that
/// reassemble to the field, with offsets counting the characters
/// before; and every segment's estimate fits the budget.
#[hegel::test(test_cases = 200)]
fn the_history_splits_into_segments_that_fit(tc: TestCase) {
    let records = tc.draw(records().print_as_debug());
    let budget =
        tc.draw(gs::integers::<usize>().min_value(150).max_value(4000));
    let segments = split_history(&records, budget).unwrap();
    assert!(!segments.is_empty());
    for segment in &segments {
        let tokens =
            estimate_state_tokens(&serde_json::to_string(segment).unwrap());
        assert!(tokens <= budget, "{tokens} > {budget}");
        assert!(!segment.is_empty() || records.is_empty());
    }
    if segments.len() > 1 {
        tc.event("several segments");
    }
    let mut placed = segments.into_iter().flatten().peekable();
    for record in &records {
        let first = placed.peek().expect("a record went missing").clone();
        if first.part.is_none() {
            assert_eq!(&first, record);
            placed.next();
            continue;
        }
        tc.event("continuations");
        for (field, id, text) in fields(record) {
            let mut joined = String::new();
            while let Some(fragment) = placed.peek() {
                let part =
                    fragment.part.expect("a whole record inside a split one");
                let (piece_field, piece_id, piece) = piece(fragment);
                if fragment.i != record.i
                    || piece_field != field
                    || piece_id != id
                {
                    break;
                }
                assert_eq!(fragment.role, record.role);
                assert_eq!(part.offset, joined.chars().count());
                assert_eq!(part.total_chars, text.chars().count());
                joined.push_str(&piece);
                placed.next();
                if joined.chars().count() == text.chars().count() {
                    break;
                }
            }
            assert_eq!(joined, text, "{field:?} of message {}", record.i);
        }
    }
    assert!(placed.next().is_none(), "a fragment belongs to no record");
}

/// Settings with drawn budgets, small enough to split.
#[hegel::composite]
fn budgets(tc: &TestCase) -> OutputPruning {
    let max_state =
        tc.draw(gs::integers::<usize>().min_value(1500).max_value(8000));
    OutputPruning {
        chunk_lines: tc
            .draw(gs::integers::<usize>().min_value(1).max_value(25)),
        max_state_tokens: max_state,
        max_request_tokens: max_state
            + tc.draw(gs::integers::<usize>().min_value(300).max_value(3000)),
        max_output_requests: tc
            .draw(gs::integers::<usize>().min_value(1).max_value(15)),
        ..OutputPruning::default()
    }
}

/// The requests for an output: none with two chunks or fewer; never more
/// than the allowance; every state fits its budget, and so does every
/// request with its questions; the first and last chunks are never
/// asked about; a chunk asked about is in the state and is asked about
/// once in every segment; and each state holds one segment, the
/// segments making up the whole history in order.
#[hegel::test(test_cases = 200)]
fn requests_fit_and_stay_within_the_allowance(tc: TestCase) {
    let text = tc.draw(output_text());
    let history = tc.draw(records().print_as_debug());
    let settings = tc.draw(budgets().print_as_debug());
    let lines = output::lines(&text);
    let planned = output::plan_requests(
        &lines,
        &history,
        "fix the build",
        "cargo build",
        &settings,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let chunks = output::chunks(lines.len(), settings.chunk_lines);
    let Some(planned) = planned else {
        assert!(chunks.len() <= 2);
        return;
    };
    assert_eq!(planned.chunks, chunks);
    assert!(planned.requests.len() <= settings.max_output_requests);
    if planned.requests.is_empty() {
        tc.event("nothing to ask within the allowance");
    }
    let last = chunks.len() - 1;
    let mut asked: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
    let mut histories: BTreeMap<usize, serde_json::Value> = BTreeMap::new();
    for (request, segment, batch) in &planned.requests {
        let state = serde_json::to_string(&request.state).unwrap();
        let questions = serde_json::to_string(&request.questions).unwrap();
        let tokens = estimate_state_tokens(&state);
        assert!(tokens <= settings.max_state_tokens);
        assert!(
            tokens
                + estimate_state_tokens(&questions)
                + REQUEST_OVERHEAD_TOKENS
                <= settings.max_request_tokens
        );
        let in_state: BTreeSet<String> = request.state["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|chunk| chunk["id"].as_str().unwrap().to_owned())
            .collect();
        let ids: Vec<String> =
            batch.iter().map(|&chunk| output::chunk_id(chunk)).collect();
        assert_eq!(request.questions.keys().cloned().collect::<Vec<_>>(), {
            let mut sorted = ids.clone();
            sorted.sort();
            sorted
        });
        for (&chunk, id) in batch.iter().zip(&ids) {
            assert!(chunk != 0 && chunk != last);
            assert!(in_state.contains(id));
            assert!(asked.entry(chunk).or_default().insert(*segment));
        }
        histories.insert(*segment, request.state["history"].clone());
    }
    for segments in asked.values() {
        assert_eq!(segments.len(), planned.segments);
    }
    if planned.segments > 1 && !asked.is_empty() {
        tc.event("several segments");
    }
    if histories.len() == planned.segments {
        let flattened: Vec<usize> = histories
            .values()
            .flat_map(|history| history.as_array().unwrap().iter())
            .map(|record| record["i"].as_u64().unwrap() as usize)
            .collect::<Vec<_>>();
        let mut ids = flattened.clone();
        ids.dedup();
        assert_eq!(ids, history.iter().map(|r| r.i).collect::<Vec<_>>());
    }
}

/// Asking sends every planned request, charges each answered one even
/// when another fails, and fails as a whole on any failure. With every
/// answer given, a chunk goes exactly when [`output::keep`] says.
#[test]
fn asking_charges_every_answer_and_fails_on_any_failure() {
    let text = (0..400)
        .map(|n| format!("step {n} of the build"))
        .collect::<Vec<_>>()
        .join("\n");
    let lines = output::lines(&text);
    let settings = OutputPruning {
        max_state_tokens: 2000,
        max_request_tokens: 2600,
        max_output_requests: 50,
        ..OutputPruning::default()
    };
    let planned = output::plan_requests(&lines, &[], "task", "make", &settings)
        .unwrap()
        .unwrap();
    assert!(planned.requests.len() > 1, "{}", planned.requests.len());

    let charged = std::sync::Mutex::new(0usize);
    let jev = FakeJev::nouls(|id| if id == "c2" { 0.9 } else { 0.0 });
    let asked = block_on(output::ask(&jev, &planned, 0.5, |_| {
        *charged.lock().unwrap() += 1;
    }))
    .unwrap();
    assert_eq!(jev.requests().len(), planned.requests.len());
    assert_eq!(*charged.lock().unwrap(), planned.requests.len());
    let dropped: Vec<usize> = (0..planned.chunks.len())
        .filter(|&chunk| !asked.keep[chunk])
        .collect();
    let expected: Vec<usize> = (2..planned.chunks.len() - 1).collect();
    assert_eq!(dropped, expected);

    let seen = std::sync::Mutex::new(0usize);
    let failing = FakeJev::new(move |request| {
        let mut seen = seen.lock().unwrap();
        *seen += 1;
        if *seen == 2 {
            return Err(JevError::Status(503));
        }
        let answers = request
            .questions
            .keys()
            .map(|id| (id.clone(), Answer::Noul { noul: 0.0 }))
            .collect();
        Ok(tau_jev::fake::response(answers, request))
    });
    let charged = std::sync::Mutex::new(0usize);
    let result = block_on(output::ask(&failing, &planned, 0.5, |_| {
        *charged.lock().unwrap() += 1;
    }));
    assert_eq!(
        result.unwrap_err().to_string(),
        "Jev answered with status 503"
    );
    assert_eq!(*charged.lock().unwrap(), planned.requests.len() - 1);
}
