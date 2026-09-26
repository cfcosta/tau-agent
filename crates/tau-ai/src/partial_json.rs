//! Incremental parsing of streamed JSON object arguments.
//!
//! OpenAI streams a tool call's JSON arguments as text deltas. Re-parsing
//! the whole buffer on every delta is quadratic in the number of deltas.
//! [`PartialJson`] instead processes each character exactly once: it keeps
//! a small lexer state machine plus a stack that describes the path to
//! the container currently being filled, and mutates a `serde_json::Value`
//! tree in place as characters arrive. It never rebuilds or re-parses the
//! buffer.
//!
//! # The partial view
//!
//! [`PartialJson::value`] returns the best-effort value built so far. The
//! root must be a JSON object (anything else is an error at
//! [`PartialJson::finish`]); before the opening `{` is seen, `value()` is
//! an empty object. From there:
//!
//! - An object member appears once its key is complete, the `:` was seen,
//!   and its value has *started* in a displayable way.
//! - A string value appears immediately (as `""`) and grows in place as
//!   characters arrive. An escape sequence split across chunks is not
//!   shown until it completes.
//! - Numbers appear only once terminated (by `,`, `}`, `]`, whitespace, or
//!   at `finish`), because `1` could still become `12`.
//! - `true`, `false` and `null` appear only once fully matched.
//! - Arrays and nested objects appear as soon as they open; their
//!   elements follow the same rules, recursively.
//!
//! Leniency matches pi's `repairJson` (`packages/ai/src/utils/json-parse.ts`):
//! inside strings, raw control characters (U+0000-U+001F) are kept as-is,
//! and a backslash followed by a character that is not a valid JSON escape
//! (or by fewer than 4 hex digits after `\u`) is kept literally as that
//! backslash and character. Outside strings, input must be valid JSON.
//!
//! Duplicate keys: like `serde_json`, the last one wins at `finish`. The
//! partial view updates a duplicate key's value as soon as its new value
//! starts, so it never shows a value that has already been superseded;
//! generated test data never has duplicate keys, so this is not covered
//! by a property.

use serde_json::{Map, Number, Value};

/// An incremental parser for a stream of JSON object text.
///
/// See the [module documentation](self) for the exact semantics of the
/// partial view.
pub struct PartialJson {
    /// The best-effort value built so far. Always `Value::Object`.
    root: Value,
    /// One entry per currently open object or array, innermost last.
    /// Each frame is a live piece of the state machine: it says what
    /// kind of token is expected next inside that container.
    frames: Vec<Frame>,
    /// The token currently being read as a value (as opposed to an
    /// object key, which buffers locally in `ObjectState::ReadingKey`).
    /// `None` when the next character starts a new value or a
    /// structural token (`,`, `:`, `}`, `]`).
    scalar: Option<ScalarState>,
    /// Characters decoded for the in-progress string value (not key)
    /// that have not yet been written into `root`. Flushed at the end
    /// of every [`PartialJson::push`] call and whenever the string
    /// closes, so a navigation from `root` to the active container
    /// happens once per batch of characters rather than once per
    /// character.
    pending: String,
    /// Whether the root `{` has been consumed.
    started: bool,
    /// Set once the input is malformed outside of a string. Further
    /// characters are ignored; `finish` returns this.
    error: Option<PartialJsonError>,
}

/// One open object or array.
enum Frame {
    Object {
        /// The key of the member currently being filled, from the
        /// moment its string closes until its value is complete.
        current_key: Option<String>,
        state: ObjectState,
    },
    Array {
        state: ArrayState,
    },
}

#[derive(Debug, PartialEq)]
enum ObjectState {
    /// Just opened, or after a value: a key may start, or `}` may close.
    KeyOrClose,
    /// After a comma: a key must start; `}` is not valid here.
    Key,
    /// Reading a key string.
    ReadingKey(StringLexer, String),
    /// The key is complete; `:` must come next.
    Colon,
    /// The colon is consumed; a value must start next.
    Value,
    /// A value is complete (or is being read as `scalar`/via `pending`);
    /// `,` or `}` comes next.
    CommaOrClose,
}

#[derive(Debug, PartialEq)]
enum ArrayState {
    /// Just opened: a value may start, or `]` may close.
    ValueOrClose,
    /// After a comma: a value must start; `]` is not valid here.
    Value,
    /// A value is complete; `,` or `]` comes next.
    CommaOrClose,
}

/// A cheap, comparable tag for the state of the top frame, used to pick
/// a dispatch arm without holding a borrow of `self.frames` across a
/// call that needs the rest of `self`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FrameTag {
    ObjAwaitKeyOrClose,
    ObjAwaitKey,
    ObjInKey,
    ObjAwaitColon,
    ObjAwaitValue,
    ObjAwaitCommaOrClose,
    ArrAwaitValueOrClose,
    ArrAwaitValue,
    ArrAwaitCommaOrClose,
}

fn frame_tag(frame: &Frame) -> FrameTag {
    match frame {
        Frame::Object { state, .. } => match state {
            ObjectState::KeyOrClose => FrameTag::ObjAwaitKeyOrClose,
            ObjectState::Key => FrameTag::ObjAwaitKey,
            ObjectState::ReadingKey(..) => FrameTag::ObjInKey,
            ObjectState::Colon => FrameTag::ObjAwaitColon,
            ObjectState::Value => FrameTag::ObjAwaitValue,
            ObjectState::CommaOrClose => FrameTag::ObjAwaitCommaOrClose,
        },
        Frame::Array { state } => match state {
            ArrayState::ValueOrClose => FrameTag::ArrAwaitValueOrClose,
            ArrayState::Value => FrameTag::ArrAwaitValue,
            ArrayState::CommaOrClose => FrameTag::ArrAwaitCommaOrClose,
        },
    }
}

/// A value being read, buffered until it can be shown (or, for strings,
/// already visible in the tree and growing via `pending`).
enum ScalarState {
    String(StringLexer),
    /// The number's text so far; not yet inserted anywhere.
    Number(String),
    /// `true`, `false` or `null`, matched one character at a time.
    Literal {
        expected: &'static str,
        matched: usize,
    },
}

/// Escape-sequence state for a string being read. Used the same way for
/// object keys (buffered locally) and value strings (buffered in
/// [`PartialJson::pending`]).
#[derive(Debug, PartialEq)]
enum StringLexer {
    /// Not mid-escape: ordinary characters, including raw control
    /// characters (leniency), go straight into the buffer.
    Plain,
    /// Just consumed a `\`. `high` carries a pending high surrogate
    /// when this backslash is expected to start its low-surrogate pair.
    Escape { high: Option<u16> },
    /// Consumed `\u` and zero or more hex digits.
    Unicode { digits: String, high: Option<u16> },
    /// A complete high surrogate was read; only a `\` starting its low
    /// surrogate pair may follow.
    AwaitLowSurrogate { high: u16 },
}

/// The outcome of feeding one character to the state machine.
enum Outcome {
    Consumed,
    /// A value completed on a character that is not part of it (a
    /// number's terminator): feed the same character again now that
    /// the scalar is no longer active.
    Reprocess,
    Error(PartialJsonError),
}

/// The outcome of feeding one character to a value that is being read
/// as `PartialJson::scalar`.
enum ScalarEvent {
    Continue,
    /// The string closed; its content is already in the tree via
    /// `pending`, which the caller must flush.
    StringClosed,
    /// A literal or number completed; `c` was fully consumed by it.
    Complete(Value),
    /// A number completed on a character that is not part of it.
    CompleteAndReprocess(Value),
    Error(PartialJsonError),
}

/// An error from [`PartialJson::finish`].
#[derive(Debug, Clone, PartialEq)]
pub enum PartialJsonError {
    /// The root value is not a JSON object.
    RootNotObject,
    /// Non-whitespace content followed the root object's closing `}`.
    TrailingGarbage,
    /// The input ended before the value was complete.
    UnexpectedEof,
    /// A character is not valid JSON syntax at this position.
    UnexpectedChar(char),
    /// A number's buffered text is not a valid JSON number.
    InvalidNumber(String),
    /// A `\uXXXX` high surrogate was not followed by a matching low
    /// surrogate escape.
    LoneSurrogate,
}

impl std::fmt::Display for PartialJsonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RootNotObject => {
                write!(f, "the root value must be a JSON object")
            }
            Self::TrailingGarbage => {
                write!(f, "unexpected content after the root object")
            }
            Self::UnexpectedEof => write!(f, "unexpected end of input"),
            Self::UnexpectedChar(c) => write!(f, "unexpected character {c:?}"),
            Self::InvalidNumber(text) => write!(f, "invalid number: {text:?}"),
            Self::LoneSurrogate => {
                write!(f, "unpaired surrogate in a \\u escape")
            }
        }
    }
}

impl std::error::Error for PartialJsonError {}

impl Default for PartialJson {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialJson {
    /// Creates a parser with nothing pushed yet; `value()` is `{}`.
    pub fn new() -> Self {
        Self {
            root: Value::Object(Map::new()),
            frames: Vec::new(),
            scalar: None,
            pending: String::new(),
            started: false,
            error: None,
        }
    }

    /// Feeds the next chunk of streamed text. Amortized `O(chunk.len() +
    /// depth)`: each character is classified once, and the value tree is
    /// navigated (from the root down to the container currently being
    /// filled) only at structural events, not once per character.
    ///
    /// Once malformed input has been seen, further chunks are ignored;
    /// call [`PartialJson::finish`] to observe the error.
    pub fn push(&mut self, chunk: &str) {
        if self.error.is_some() {
            return;
        }
        for c in chunk.chars() {
            if !self.feed_char(c) {
                break;
            }
        }
        self.flush_pending();
    }

    /// The current best-effort value. See the [module documentation](self)
    /// for exactly what is visible at each point.
    pub fn value(&self) -> &Value {
        &self.root
    }

    /// Consumes the parser and returns the complete value, or the error
    /// that made the input invalid JSON.
    pub fn finish(mut self) -> Result<Value, PartialJsonError> {
        if let Some(err) = self.error {
            return Err(err);
        }
        // A trailing number has no terminator to reveal that it was
        // already complete; resolve it now, at the true end of input.
        if matches!(self.scalar, Some(ScalarState::Number(_))) {
            let Some(ScalarState::Number(text)) = self.scalar.take() else {
                unreachable!("just matched Some(ScalarState::Number(_))")
            };
            match serde_json::from_str::<Number>(&text) {
                Ok(n) => {
                    self.place_value(Value::Number(n));
                    self.finish_current_value();
                }
                Err(_) => return Err(PartialJsonError::InvalidNumber(text)),
            }
        }
        if self.scalar.is_some() {
            // Mid-string or mid-literal: neither has a valid end here.
            return Err(PartialJsonError::UnexpectedEof);
        }
        if !self.started || !self.frames.is_empty() {
            return Err(PartialJsonError::UnexpectedEof);
        }
        Ok(self.root)
    }

    /// Feeds one character through the state machine, looping when a
    /// value completes on a character that belongs to what follows it
    /// (a number's terminator). Returns `false` once an error is set,
    /// so the caller can stop scanning the rest of the chunk.
    fn feed_char(&mut self, c: char) -> bool {
        loop {
            let outcome = if self.scalar.is_some() {
                match self.feed_scalar_char(c) {
                    ScalarEvent::Continue => Outcome::Consumed,
                    ScalarEvent::StringClosed => {
                        self.flush_pending();
                        self.finish_current_value();
                        self.scalar = None;
                        Outcome::Consumed
                    }
                    ScalarEvent::Complete(value) => {
                        self.place_value(value);
                        self.finish_current_value();
                        self.scalar = None;
                        Outcome::Consumed
                    }
                    ScalarEvent::CompleteAndReprocess(value) => {
                        self.place_value(value);
                        self.finish_current_value();
                        self.scalar = None;
                        Outcome::Reprocess
                    }
                    ScalarEvent::Error(e) => Outcome::Error(e),
                }
            } else {
                self.feed_structural_char(c)
            };
            match outcome {
                Outcome::Consumed => return true,
                Outcome::Reprocess => continue,
                Outcome::Error(e) => {
                    self.error = Some(e);
                    return false;
                }
            }
        }
    }

    /// Feeds a character to the in-progress `scalar` (string, number or
    /// literal). Only touches `self.scalar` and `self.pending`, so it
    /// never conflicts with the wider mutations its result requires.
    fn feed_scalar_char(&mut self, c: char) -> ScalarEvent {
        match self
            .scalar
            .as_mut()
            .expect("called only while scalar.is_some()")
        {
            ScalarState::String(lexer) => {
                match feed_string_char(lexer, &mut self.pending, c) {
                    Ok(false) => ScalarEvent::Continue,
                    Ok(true) => ScalarEvent::StringClosed,
                    Err(e) => ScalarEvent::Error(e),
                }
            }
            ScalarState::Number(buf) => {
                if is_number_char(c) {
                    buf.push(c);
                    ScalarEvent::Continue
                } else if is_number_terminator(c) {
                    match serde_json::from_str::<Number>(buf) {
                        Ok(n) => {
                            ScalarEvent::CompleteAndReprocess(Value::Number(n))
                        }
                        Err(_) => ScalarEvent::Error(
                            PartialJsonError::InvalidNumber(buf.clone()),
                        ),
                    }
                } else {
                    ScalarEvent::Error(PartialJsonError::InvalidNumber(
                        buf.clone(),
                    ))
                }
            }
            ScalarState::Literal { expected, matched } => {
                if expected.as_bytes()[*matched] as char == c {
                    *matched += 1;
                    if *matched == expected.len() {
                        ScalarEvent::Complete(literal_value(expected))
                    } else {
                        ScalarEvent::Continue
                    }
                } else {
                    ScalarEvent::Error(PartialJsonError::UnexpectedChar(c))
                }
            }
        }
    }

    /// Feeds a character that is not part of an in-progress scalar:
    /// whitespace, structural punctuation, a key string, or the first
    /// character of a new value.
    fn feed_structural_char(&mut self, c: char) -> Outcome {
        let Some(top) = self.frames.last() else {
            return self.feed_top_level_char(c);
        };
        let tag = frame_tag(top);
        if tag != FrameTag::ObjInKey && is_json_whitespace(c) {
            return Outcome::Consumed;
        }
        match tag {
            FrameTag::ObjAwaitKeyOrClose => match c {
                '"' => {
                    self.set_obj_state(ObjectState::ReadingKey(
                        StringLexer::Plain,
                        String::new(),
                    ));
                    Outcome::Consumed
                }
                '}' => {
                    self.pop_frame();
                    Outcome::Consumed
                }
                _ => Outcome::Error(PartialJsonError::UnexpectedChar(c)),
            },
            FrameTag::ObjAwaitKey => match c {
                '"' => {
                    self.set_obj_state(ObjectState::ReadingKey(
                        StringLexer::Plain,
                        String::new(),
                    ));
                    Outcome::Consumed
                }
                _ => Outcome::Error(PartialJsonError::UnexpectedChar(c)),
            },
            FrameTag::ObjInKey => {
                let (result, key) = {
                    let Some(Frame::Object {
                        state: ObjectState::ReadingKey(lexer, buf),
                        ..
                    }) = self.frames.last_mut()
                    else {
                        unreachable!("tag == ObjInKey")
                    };
                    let result = feed_string_char(lexer, buf, c);
                    let key =
                        matches!(result, Ok(true)).then(|| std::mem::take(buf));
                    (result, key)
                };
                match result {
                    Ok(false) => Outcome::Consumed,
                    Ok(true) => {
                        self.finish_key(
                            key.expect("Ok(true) implies key is Some"),
                        );
                        Outcome::Consumed
                    }
                    Err(e) => Outcome::Error(e),
                }
            }
            FrameTag::ObjAwaitColon => match c {
                ':' => {
                    self.set_obj_state(ObjectState::Value);
                    Outcome::Consumed
                }
                _ => Outcome::Error(PartialJsonError::UnexpectedChar(c)),
            },
            FrameTag::ObjAwaitValue => self.begin_value(c),
            FrameTag::ObjAwaitCommaOrClose => match c {
                ',' => {
                    self.set_obj_state(ObjectState::Key);
                    Outcome::Consumed
                }
                '}' => {
                    self.pop_frame();
                    Outcome::Consumed
                }
                _ => Outcome::Error(PartialJsonError::UnexpectedChar(c)),
            },
            FrameTag::ArrAwaitValueOrClose => match c {
                ']' => {
                    self.pop_frame();
                    Outcome::Consumed
                }
                _ => self.begin_value(c),
            },
            FrameTag::ArrAwaitValue => self.begin_value(c),
            FrameTag::ArrAwaitCommaOrClose => match c {
                ',' => {
                    self.set_arr_state(ArrayState::Value);
                    Outcome::Consumed
                }
                ']' => {
                    self.pop_frame();
                    Outcome::Consumed
                }
                _ => Outcome::Error(PartialJsonError::UnexpectedChar(c)),
            },
        }
    }

    /// Dispatch used while no container is open: before the root `{`,
    /// and after it closes.
    fn feed_top_level_char(&mut self, c: char) -> Outcome {
        if !self.started {
            if is_json_whitespace(c) {
                return Outcome::Consumed;
            }
            if c == '{' {
                self.started = true;
                self.frames.push(Frame::Object {
                    current_key: None,
                    state: ObjectState::KeyOrClose,
                });
                return Outcome::Consumed;
            }
            Outcome::Error(PartialJsonError::RootNotObject)
        } else if is_json_whitespace(c) {
            Outcome::Consumed
        } else {
            Outcome::Error(PartialJsonError::TrailingGarbage)
        }
    }

    /// Handles the first character of a value: decides its kind and
    /// either opens it (strings and containers, which are displayable
    /// immediately) or starts buffering it (numbers and literals, which
    /// are not shown until complete).
    ///
    /// Note this does not itself move the frame's state to
    /// `CommaOrClose`: while a value is in progress, the frame is either
    /// not `frames.last()` (a nested container was pushed on top of it)
    /// or is bypassed entirely by the `scalar` dispatch in `feed_char`,
    /// so its `state` field is not read again until
    /// [`Self::finish_current_value`] sets it once the value actually
    /// completes.
    fn begin_value(&mut self, c: char) -> Outcome {
        match c {
            '{' => {
                self.place_value(Value::Object(Map::new()));
                self.frames.push(Frame::Object {
                    current_key: None,
                    state: ObjectState::KeyOrClose,
                });
                Outcome::Consumed
            }
            '[' => {
                self.place_value(Value::Array(Vec::new()));
                self.frames.push(Frame::Array {
                    state: ArrayState::ValueOrClose,
                });
                Outcome::Consumed
            }
            '"' => {
                self.place_value(Value::String(String::new()));
                self.scalar = Some(ScalarState::String(StringLexer::Plain));
                Outcome::Consumed
            }
            '-' | '0'..='9' => {
                self.scalar = Some(ScalarState::Number(c.to_string()));
                Outcome::Consumed
            }
            't' => {
                self.scalar = Some(ScalarState::Literal {
                    expected: "true",
                    matched: 1,
                });
                Outcome::Consumed
            }
            'f' => {
                self.scalar = Some(ScalarState::Literal {
                    expected: "false",
                    matched: 1,
                });
                Outcome::Consumed
            }
            'n' => {
                self.scalar = Some(ScalarState::Literal {
                    expected: "null",
                    matched: 1,
                });
                Outcome::Consumed
            }
            _ => Outcome::Error(PartialJsonError::UnexpectedChar(c)),
        }
    }

    fn set_obj_state(&mut self, new_state: ObjectState) {
        if let Some(Frame::Object { state, .. }) = self.frames.last_mut() {
            *state = new_state;
        }
    }

    fn set_arr_state(&mut self, new_state: ArrayState) {
        if let Some(Frame::Array { state }) = self.frames.last_mut() {
            *state = new_state;
        }
    }

    /// Stores a key that just finished reading, and awaits its `:`.
    fn finish_key(&mut self, key: String) {
        if let Some(Frame::Object { current_key, state }) =
            self.frames.last_mut()
        {
            *current_key = Some(key);
            *state = ObjectState::Colon;
        }
    }

    /// Marks the current value complete: clears the object's pending
    /// key, if any, and returns the frame to `CommaOrClose`.
    fn finish_current_value(&mut self) {
        match self
            .frames
            .last_mut()
            .expect("a value implies an open frame")
        {
            Frame::Object { current_key, state } => {
                *current_key = None;
                *state = ObjectState::CommaOrClose;
            }
            Frame::Array { state } => *state = ArrayState::CommaOrClose,
        }
    }

    /// Closes the top frame. If a container remains open below it, that
    /// container's pending member/element is now complete.
    fn pop_frame(&mut self) {
        self.frames.pop();
        if !self.frames.is_empty() {
            self.finish_current_value();
        }
    }

    /// Navigates from `root` to the container the top frame represents
    /// (not into its own pending child). `O(depth)`.
    fn active_container_mut(&mut self) -> &mut Value {
        let mut value = &mut self.root;
        let ancestors = self.frames.len().saturating_sub(1);
        for frame in &self.frames[..ancestors] {
            value = match frame {
                Frame::Object { current_key, .. } => value
                    .as_object_mut()
                    .expect("an object frame maps to an object value")
                    .get_mut(current_key.as_ref().expect(
                        "descending through a frame with an open value",
                    ))
                    .expect("the pending value was already inserted"),
                Frame::Array { .. } => value
                    .as_array_mut()
                    .expect("an array frame maps to an array value")
                    .last_mut()
                    .expect("the pending element was already inserted"),
            };
        }
        value
    }

    /// Inserts `value` at the top frame's pending slot (the object's
    /// current key, or the array's next index). For an object, the key
    /// is read, not taken: a string or container value must still be
    /// reachable by key while it grows.
    fn place_value(&mut self, value: Value) {
        let key = match self.frames.last() {
            Some(Frame::Object { current_key, .. }) => current_key.clone(),
            _ => None,
        };
        match self.active_container_mut() {
            Value::Object(map) => {
                map.insert(
                    key.expect("an object frame has a pending key"),
                    value,
                );
            }
            Value::Array(vec) => vec.push(value),
            _ => {
                unreachable!("a frame's container is always an object or array")
            }
        }
    }

    /// Appends `self.pending` to the in-progress string value and clears
    /// it. A no-op when nothing is buffered, which is the common case
    /// when called unconditionally at the end of `push`.
    fn flush_pending(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        let key = match self.frames.last() {
            Some(Frame::Object { current_key, .. }) => current_key.clone(),
            _ => None,
        };
        let pending = std::mem::take(&mut self.pending);
        let container = self.active_container_mut();
        let target = match (container, key) {
            (Value::Object(map), Some(k)) => map
                .get_mut(&k)
                .expect("the string value was already inserted"),
            (Value::Array(vec), None) => vec
                .last_mut()
                .expect("the string value was already inserted"),
            _ => {
                unreachable!("a frame's container is always an object or array")
            }
        };
        let Value::String(s) = target else {
            unreachable!(
                "pending only accumulates for an in-progress string value"
            )
        };
        s.push_str(&pending);
    }
}

fn is_json_whitespace(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn is_number_char(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-')
}

fn is_number_terminator(c: char) -> bool {
    is_json_whitespace(c) || matches!(c, ',' | '}' | ']')
}

fn literal_value(expected: &str) -> Value {
    match expected {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" => Value::Null,
        _ => unreachable!(
            "the only literals begin_value starts are true/false/null"
        ),
    }
}

/// Feeds one character to a string lexer, appending decoded characters
/// to `buf`. Returns `Ok(true)` once the closing `"` is consumed.
///
/// Leniency (matching pi's `repairJson`): raw control characters are
/// kept as-is; a backslash followed by anything other than a valid JSON
/// escape, or by fewer than 4 hex digits after `\u`, is kept literally
/// as that backslash and the characters seen so far. A high surrogate
/// that is not followed by a valid low-surrogate escape has no way to
/// be represented in a Rust `String`, so it is always an error, even
/// under leniency.
fn feed_string_char(
    lexer: &mut StringLexer,
    buf: &mut String,
    c: char,
) -> Result<bool, PartialJsonError> {
    match lexer {
        StringLexer::Plain => match c {
            '"' => Ok(true),
            '\\' => {
                *lexer = StringLexer::Escape { high: None };
                Ok(false)
            }
            _ => {
                buf.push(c);
                Ok(false)
            }
        },
        StringLexer::AwaitLowSurrogate { high } => {
            if c == '\\' {
                *lexer = StringLexer::Escape { high: Some(*high) };
                Ok(false)
            } else {
                Err(PartialJsonError::LoneSurrogate)
            }
        }
        StringLexer::Escape { high } => {
            let high = *high;
            let decoded = match c {
                '"' => Some('"'),
                '\\' => Some('\\'),
                '/' => Some('/'),
                'b' => Some('\u{8}'),
                'f' => Some('\u{c}'),
                'n' => Some('\n'),
                'r' => Some('\r'),
                't' => Some('\t'),
                'u' => {
                    *lexer = StringLexer::Unicode {
                        digits: String::new(),
                        high,
                    };
                    return Ok(false);
                }
                _ => None,
            };
            if high.is_some() {
                // A high surrogate must be followed by a `\u` escape for
                // its low half; nothing else can represent it.
                return Err(PartialJsonError::LoneSurrogate);
            }
            match decoded {
                Some(ch) => buf.push(ch),
                None => {
                    // Leniency: keep the backslash and this character.
                    buf.push('\\');
                    buf.push(c);
                }
            }
            *lexer = StringLexer::Plain;
            Ok(false)
        }
        StringLexer::Unicode { digits, high } => {
            // `digits` is never already 4 long on entry here: the
            // moment it reaches 4 (below), `resolve_unicode_escape`
            // either moves `*lexer` away from `Unicode` (success) or
            // returns `Err`, which poisons the whole parser (`push`
            // stops scanning, and `finish` reports the error) before
            // this state could ever be read again. So there is no
            // length bound left to check on entry.
            if c.is_ascii_hexdigit() {
                digits.push(c);
                if digits.len() == 4 {
                    let high = *high;
                    resolve_unicode_escape(lexer, buf, high)?;
                }
                Ok(false)
            } else if high.is_some() {
                Err(PartialJsonError::LoneSurrogate)
            } else {
                // Leniency: fewer than 4 hex digits. Keep `\u` and the
                // digits seen so far literally, then reprocess `c`.
                let digits = std::mem::take(digits);
                buf.push('\\');
                buf.push('u');
                buf.push_str(&digits);
                *lexer = StringLexer::Plain;
                feed_string_char(lexer, buf, c)
            }
        }
    }
}

/// Resolves a completed 4-digit `\uXXXX` escape once its state has 4
/// hex digits, combining it with a pending high surrogate if there is
/// one. Leaves `*lexer` as `Plain` on success, or `AwaitLowSurrogate` if
/// this was a high surrogate starting a new pair.
fn resolve_unicode_escape(
    lexer: &mut StringLexer,
    buf: &mut String,
    high: Option<u16>,
) -> Result<(), PartialJsonError> {
    let StringLexer::Unicode { digits, .. } = lexer else {
        unreachable!("called only from the Unicode arm")
    };
    let value = u16::from_str_radix(digits, 16).expect("4 checked hex digits");
    match high {
        Some(high) => {
            if !(0xDC00..=0xDFFF).contains(&value) {
                return Err(PartialJsonError::LoneSurrogate);
            }
            let combined = 0x10000
                + (u32::from(high) - 0xD800) * 0x400
                + (u32::from(value) - 0xDC00);
            buf.push(char::from_u32(combined).expect("a valid surrogate pair"));
            *lexer = StringLexer::Plain;
            Ok(())
        }
        None => {
            if (0xD800..=0xDBFF).contains(&value) {
                *lexer = StringLexer::AwaitLowSurrogate { high: value };
                Ok(())
            } else if (0xDC00..=0xDFFF).contains(&value) {
                Err(PartialJsonError::LoneSurrogate)
            } else {
                buf.push(
                    char::from_u32(u32::from(value)).expect("not a surrogate"),
                );
                *lexer = StringLexer::Plain;
                Ok(())
            }
        }
    }
}
