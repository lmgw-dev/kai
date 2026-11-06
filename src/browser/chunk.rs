//! Frames in pieces (kai-firefox design §4.1, §4.2). Firefox takes at most
//! [`super::frame::TO_FIREFOX_MAX`] in one message toward the extension, so Kai sends anything
//! longer as `chunk` frames and an `end`, which the background joins. That is framing, not a cap:
//! the pieces join to exactly what was given. The extension splits its long frames toward Kai the
//! same way (`split`; Firefox writes one large message slowly, measured in Firefox 159: 50 MiB
//! whole 11 s, as 1 MiB pieces 0.2 s), and [`Joiner`] joins them.
//!
//! Two uses, told apart by the key a chunk frame carries:
//! - **A split frame** (`split`): a frame Kai sends that is longer than the limit (an `op` whose
//!   args carry a long style sheet, script or file) goes as
//!   `{"t":"chunk","split":<n>,"seq":<i>,"data":"<a piece of the frame's JSON text>"}` frames and
//!   `{"t":"end","split":<n>,"seq":<count>}`. The background joins the `data` texts in `seq` order
//!   and reads the result as one frame. `<n>` counts a link's split frames from 1. A link's writer
//!   does this for every frame it sends (`super::server`), so no sender thinks of it, and a split
//!   frame's pieces go out one after the other.
//! - **A body** (`id`): a `res` frame (the sidebar's proxy, design §8.2) is followed by its body as
//!   `{"t":"chunk","id":<the req's id>,"seq":<i>,"data":…}` frames and
//!   `{"t":"end","id":…,"seq":<count>}`. The `res` frame says how `data` reads: `"body": "text"`
//!   (the pieces join as text: JSON, a stream of events) or `"body": "base64"` (each piece's `data`
//!   decodes on its own to bytes, which join: [`Chunker::bytes_piece`] cuts whole 3-byte groups, and
//!   only the last piece of what it is given at once is padded: of a whole body, its last piece; of
//!   a stream, the last piece of each of lmgw's chunks). A piece of a body may be any size:
//!   [`Chunker`] cuts it into frames within the limit.
//!
//! A chunk frame, as serialized, is never longer than the limit. Text is cut between characters,
//! and its length counts JSON's escapes (a `"` of the frame's text takes two bytes in `data`). A
//! key that leaves no room for data in a frame within the limit (a request's id of about the
//! limit's length) is an error ([`NoRoom`]), which the sender answers with an `error` frame.
//!
//! **Joining the extension's splits** ([`Joiner`]): a split's pieces come one after the other,
//! `seq` from 0, and its `end`'s `seq` is their count; the joined text is read as one frame (of
//! any type but `chunk` and `end`) of any length. Anything else is an error ([`BadSplit`],
//! answered with an `error` frame) that drops the split, whose later pieces and end are then read
//! past quietly: a piece out of order, an `end` whose count is not the pieces', another frame or
//! split between the pieces, a piece of no split that began, text that is no JSON object. A split
//! still open when the link closes is dropped with it.

use serde_json::{Map, Value};

/// What a chunk frame belongs to (module doc).
#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    /// A body after a `res` frame: the request's id.
    Id(Value),
    /// A frame too long for one message: the link's count of split frames.
    Split(u64),
}

impl Key {
    fn field(&self) -> (&'static str, Value) {
        match self {
            Key::Id(id) => ("id", id.clone()),
            Key::Split(n) => ("split", Value::from(*n)),
        }
    }
}

/// The most bytes one character takes in a JSON string (`\u001f`).
const WIDEST: usize = 6;

/// The bytes `c` takes inside a JSON string as serde_json writes it: `"` and `\` escaped by a
/// backslash, the control characters as `\n`-style or `\u00XX` escapes, every other character as
/// its UTF-8.
fn escaped_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
        c if (c as u32) < 0x20 => WIDEST,
        c => c.len_utf8(),
    }
}

/// A chunk frame's key takes so much of the limit that no data fits beside it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "a chunk frame's key takes {key} bytes, which leaves no room for data in a frame of at most \
     {limit} bytes"
)]
pub struct NoRoom {
    /// The bytes a chunk frame with this key and no data takes.
    pub key: usize,
    pub limit: usize,
}

/// The chunk frames of one key, numbered as they are made.
pub struct Chunker {
    key: Key,
    seq: u64,
    limit: usize,
}

impl Chunker {
    pub fn new(key: Key, limit: usize) -> Self {
        Self { key, seq: 0, limit }
    }

    fn frame(&self, data: &str) -> String {
        let (name, value) = self.key.field();
        let mut m = Map::new();
        m.insert("t".into(), "chunk".into());
        m.insert(name.into(), value);
        m.insert("seq".into(), self.seq.into());
        m.insert("data".into(), data.into());
        Value::Object(m).to_string()
    }

    /// The room `data` has in the next frame: at least one character's widest escape.
    fn room(&self) -> Result<usize, NoRoom> {
        let empty = self.frame("").len();
        match self.limit.checked_sub(empty) {
            Some(room) if room >= WIDEST => Ok(room),
            _ => Err(NoRoom {
                key: empty,
                limit: self.limit,
            }),
        }
    }

    /// `text` as chunk frames (none for empty text).
    pub fn text(&mut self, text: &str) -> Result<Vec<String>, NoRoom> {
        let mut out = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            out.push(self.piece(&mut rest)?);
        }
        Ok(out)
    }

    /// The next chunk frame of `rest` (not empty), which keeps what is left: a frame at a time,
    /// so a long text's frames need not all be held at once.
    pub fn piece(&mut self, rest: &mut &str) -> Result<String, NoRoom> {
        let room = self.room()?;
        let (mut used, mut cut) = (0, 0);
        for (i, c) in rest.char_indices() {
            let n = escaped_len(c);
            if used + n > room {
                break;
            }
            used += n;
            cut = i + c.len_utf8();
        }
        let (piece, tail) = rest.split_at(cut);
        let frame = self.frame(piece);
        self.seq += 1;
        *rest = tail;
        Ok(frame)
    }

    /// The next chunk frame of `rest` (not empty) as base64, which keeps what is left: as many
    /// whole 3-byte groups as the frame has room for, so its `data` decodes on its own. Only the
    /// piece that takes the end of `rest` can be shorter than a group, and padded: once for a whole
    /// body, once for each of lmgw's chunks of a stream.
    pub fn bytes_piece(&mut self, rest: &mut &[u8]) -> Result<String, NoRoom> {
        use base64::Engine as _;
        // At least WIDEST (6) bytes of room: one group of 4 characters, 3 bytes.
        let take = (self.room()? / 4 * 3).min(rest.len());
        let (piece, tail) = rest.split_at(take);
        let frame = self.frame(&base64::engine::general_purpose::STANDARD.encode(piece));
        self.seq += 1;
        *rest = tail;
        Ok(frame)
    }

    /// Whether any data fits beside this key in a frame within the limit ([`NoRoom`] when not).
    pub fn check(&self) -> Result<(), NoRoom> {
        self.room().map(|_| ())
    }

    /// The pieces made so far: the next one's `seq`.
    pub fn count(&self) -> u64 {
        self.seq
    }

    /// The `end` frame: `seq` is the number of chunk frames sent.
    pub fn end(self) -> String {
        let (name, value) = self.key.field();
        let mut m = Map::new();
        m.insert("t".into(), "end".into());
        m.insert(name.into(), value);
        m.insert("seq".into(), self.seq.into());
        Value::Object(m).to_string()
    }
}

/// `frame` (serialized) as the frames that carry it within `limit`: itself when it fits, else
/// its chunk frames under `Key::Split(split)` and the `end`. A split key is a number, so only a
/// limit of a few dozen bytes leaves it no room. (The link's writer cuts a frame a piece at a
/// time with [`Chunker::piece`], to the same frames.)
#[cfg(test)]
pub fn fit(
    frame: String,
    split: impl FnOnce() -> u64,
    limit: usize,
) -> Result<Vec<String>, NoRoom> {
    if frame.len() <= limit {
        return Ok(vec![frame]);
    }
    let mut c = Chunker::new(Key::Split(split()), limit);
    let mut out = c.text(&frame)?;
    out.push(c.end());
    Ok(out)
}

/// The extension's split frames, joined (module doc). One per link.
#[derive(Default)]
pub struct Joiner {
    open: Option<Open>,
    /// A split dropped for an error: its later pieces and its end are read past.
    dropped: Option<u64>,
}

struct Open {
    split: u64,
    /// The pieces taken, so the next one's `seq`.
    count: u64,
    text: String,
}

/// What a piece did ([`Joiner::piece`]).
#[derive(Debug, PartialEq)]
pub enum Piece {
    /// Taken (or read past, a dropped split's); the split is not complete.
    Taken,
    /// The split's `end`: the frame its pieces carried, with its `t`.
    Frame(Map<String, Value>),
}

/// A split that cannot be joined: dropped, and said in an `error` frame (`code` `bad_split`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct BadSplit {
    /// The split it is about, when the frame named one.
    pub split: Option<u64>,
    pub message: String,
}

impl Joiner {
    /// A frame of type `t` (its other fields `frame`) came: when a split is open and this is not
    /// its next piece or its end, the split is dropped, and why.
    pub fn interrupted(&mut self, t: Option<&str>, frame: &Map<String, Value>) -> Option<BadSplit> {
        let open = self.open.as_ref()?;
        let same = matches!(t, Some("chunk" | "end"))
            && frame.get("split").and_then(Value::as_u64) == Some(open.split);
        if same {
            return None;
        }
        let split = open.split;
        self.drop_open();
        let what = match (t, frame.get("split").and_then(Value::as_u64)) {
            (Some(t @ ("chunk" | "end")), Some(other)) => format!("split {other}'s {t}"),
            (Some(t), _) => format!("the extension's {t} frame"),
            (None, _) => "a frame without a type".into(),
        };
        Some(BadSplit {
            split: Some(split),
            message: format!(
                "split {split} was interrupted by {what} (a split's pieces come one after the \
                 other); it is dropped"
            ),
        })
    }

    /// A `chunk` or `end` frame (`t`; its other fields `frame`), after [`Joiner::interrupted`].
    pub fn piece(&mut self, t: &str, frame: &Map<String, Value>) -> Result<Piece, BadSplit> {
        let split = frame.get("split").and_then(Value::as_u64);
        let seq = frame.get("seq").and_then(Value::as_u64);
        let (Some(split), Some(seq)) = (split, seq) else {
            self.drop_open();
            if split.is_some() {
                self.dropped = split;
            }
            return Err(BadSplit {
                split,
                message: format!(
                    "a {t} frame from the extension needs a whole-number split and seq"
                ),
            });
        };
        if self.open.is_none() && self.dropped == Some(split) {
            if t == "end" {
                self.dropped = None;
            }
            return Ok(Piece::Taken);
        }
        let fail = |this: &mut Self, message: String| {
            this.drop_open();
            this.dropped = Some(split);
            Err(BadSplit {
                split: Some(split),
                message,
            })
        };
        if let Some(open) = self.open.as_ref().map(|o| o.split)
            && open != split
        {
            return fail(
                self,
                format!("split {split}'s {t} came while split {open} was open; both are dropped"),
            );
        }
        if t == "end" {
            let Some(o) = self.open.take() else {
                return fail(self, format!("an end of split {split}, which never began"));
            };
            if o.count != seq {
                let count = o.count;
                return fail(
                    self,
                    format!("split {split} ended at seq {seq}, and {count} pieces came"),
                );
            }
            let len = o.text.len();
            return match serde_json::from_str::<Value>(&o.text) {
                Ok(Value::Object(m))
                    if m.get("t")
                        .and_then(Value::as_str)
                        .is_some_and(|t| t != "chunk" && t != "end") =>
                {
                    Ok(Piece::Frame(m))
                }
                Ok(_) => fail(
                    self,
                    format!(
                        "split {split} joined to {len} bytes that are no frame (a JSON object with \
                         a type other than chunk and end)"
                    ),
                ),
                Err(e) => fail(
                    self,
                    format!("split {split} joined to {len} bytes that are no JSON: {e}"),
                ),
            };
        }
        let Some(data) = frame.get("data").and_then(Value::as_str) else {
            return fail(
                self,
                format!("split {split}'s piece {seq} has no text data"),
            );
        };
        match &mut self.open {
            None if seq == 0 => {
                self.open = Some(Open {
                    split,
                    count: 1,
                    text: data.to_owned(),
                });
                Ok(Piece::Taken)
            }
            None => fail(
                self,
                format!("split {split}'s piece {seq} came, and its piece 0 never did"),
            ),
            Some(o) if o.count == seq => {
                o.count += 1;
                o.text.push_str(data);
                Ok(Piece::Taken)
            }
            Some(o) => {
                let due = o.count;
                fail(
                    self,
                    format!("split {split}'s piece {seq} came where piece {due} was due"),
                )
            }
        }
    }

    /// The split open now, which a link that closes drops.
    pub fn open(&self) -> Option<u64> {
        self.open.as_ref().map(|o| o.split)
    }

    fn drop_open(&mut self) {
        if let Some(o) = self.open.take() {
            self.dropped = Some(o.split);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn read(frame: &str) -> Map<String, Value> {
        serde_json::from_str::<Value>(frame)
            .unwrap()
            .as_object()
            .unwrap()
            .clone()
    }

    /// Splits `frame` within `limit` and joins it as the background would.
    fn round_trip(frame: &str, limit: usize) -> usize {
        let frames = fit(frame.to_owned(), || 7, limit).unwrap();
        if frame.len() <= limit {
            assert_eq!(frames, [frame]);
            return 1;
        }
        let mut joined = String::new();
        let (end, chunks) = frames.split_last().unwrap();
        for (i, f) in chunks.iter().enumerate() {
            assert!(f.len() <= limit, "a chunk frame of {} > {limit}", f.len());
            let m = read(f);
            assert_eq!(m["t"], "chunk");
            assert_eq!(m["split"], 7);
            assert_eq!(m["seq"], i);
            assert!(m.get("id").is_none());
            joined.push_str(m["data"].as_str().unwrap());
        }
        let end = read(end);
        assert_eq!(end["t"], "end");
        assert_eq!(end["split"], 7);
        assert_eq!(end["seq"], chunks.len());
        assert_eq!(joined, frame, "the pieces join to the frame");
        let _: Value = serde_json::from_str(&joined).unwrap();
        frames.len()
    }

    #[test]
    fn escapes_are_counted_as_serde_json_writes_them() {
        for c in [
            'a', '"', '\\', '\n', '\t', '\u{1}', '\u{1f}', '\u{7f}', 'ä', '€', '😀', '/',
        ] {
            let written = serde_json::to_string(&c.to_string()).unwrap();
            assert_eq!(escaped_len(c), written.len() - 2, "{c:?} as {written}");
        }
    }

    #[test]
    fn a_long_frame_splits_within_the_limit_and_joins_to_itself() {
        let args = json!({
            "css": "a::before { content: \"\\201C\" } /* ä€😀 */\n\t".repeat(40),
            "bytes": "\u{1}\u{1f}\"\\".repeat(30),
        });
        let frame = json!({"t": "op", "id": 12, "op": "page.css@1", "args": args}).to_string();
        for limit in [60, 61, 97, 128, 1000, frame.len() - 1] {
            assert!(round_trip(&frame, limit) > 2, "limit {limit}");
        }
        assert_eq!(
            round_trip(&frame, frame.len()),
            1,
            "a frame that fits stays whole"
        );
        // The real limit: a 3 MiB op.
        let big = json!({"t": "op", "id": 1, "op": "element.file@1",
                         "args": {"data": "x\"".repeat(3 * 1024 * 1024 / 2)}})
        .to_string();
        // 3 MiB of `x"` is 4.5 MiB of frame text (`x\"`) and 7.5 MiB of chunk data (`x\\\"`).
        let n = round_trip(&big, crate::browser::frame::TO_FIREFOX_MAX);
        assert_eq!(n, 9, "8 chunk frames and the end");
    }

    #[test]
    fn a_body_goes_as_text_pieces_under_its_id() {
        let mut c = Chunker::new(Key::Id(json!("r1")), 80);
        let mut frames = c.text("data: {\"a\":1}\n\n").unwrap();
        frames.extend(c.text("").unwrap());
        frames.extend(c.text(&"é".repeat(100)).unwrap());
        let n = frames.len();
        frames.push(c.end());
        let mut text = String::new();
        for (i, f) in frames[..n].iter().enumerate() {
            assert!(f.len() <= 80);
            let m = read(f);
            assert_eq!(
                (m["id"].as_str(), m["seq"].as_u64()),
                (Some("r1"), Some(i as u64))
            );
            text.push_str(m["data"].as_str().unwrap());
        }
        assert_eq!(text, format!("data: {{\"a\":1}}\n\n{}", "é".repeat(100)));
        assert_eq!(read(&frames[n])["seq"], n);
    }

    /// A key about as long as the limit leaves no room for data: an error naming the sizes, not a
    /// panic; a key that leaves one escape's room still works.
    #[test]
    fn a_key_that_leaves_no_room_is_an_error() {
        let limit = 100;
        let empty = |id: &str| Chunker::new(Key::Id(json!(id)), limit).frame("").len();
        let base = empty("");
        let fits = "x".repeat(limit - base - WIDEST);
        let mut c = Chunker::new(Key::Id(json!(fits)), limit);
        let frames = c.text("\u{1}\u{1}").unwrap();
        assert_eq!(frames.len(), 2, "one escape per frame");
        assert!(frames.iter().all(|f| f.len() <= limit));

        let long = "x".repeat(limit - base - WIDEST + 1);
        let mut c = Chunker::new(Key::Id(json!(long)), limit);
        let e = c.text("a").unwrap_err();
        assert_eq!(
            e,
            NoRoom {
                key: empty(&long),
                limit
            }
        );
        assert!(e.to_string().contains("of at most 100 bytes"), "{e}");
        assert_eq!(
            c.text("").unwrap(),
            Vec::<String>::new(),
            "no text, no frame"
        );
        let huge = "x".repeat(2 * limit);
        assert!(Chunker::new(Key::Id(json!(huge)), limit).text("a").is_err());
        assert!(fit("x".repeat(50), || 1, 20).is_err(), "a split key too");
    }

    /// Feeds the joiner `frames` as the server does: the interruption check, then the piece.
    fn feed(j: &mut Joiner, frames: &[String]) -> Vec<Result<Option<Value>, BadSplit>> {
        let mut out = Vec::new();
        for f in frames {
            let mut m = read(f);
            let t = m.remove("t").and_then(|t| t.as_str().map(str::to_owned));
            if let Some(e) = j.interrupted(t.as_deref(), &m) {
                out.push(Err(e));
            }
            match t.as_deref() {
                Some(p @ ("chunk" | "end")) => match j.piece(p, &m) {
                    Ok(Piece::Taken) => {}
                    Ok(Piece::Frame(whole)) => out.push(Ok(Some(Value::Object(whole)))),
                    Err(e) => out.push(Err(e)),
                },
                _ => out.push(Ok(None)),
            }
        }
        out
    }

    /// The extension's long frame, split as Kai splits (`fit`), joins back to itself.
    #[test]
    fn a_split_from_the_extension_joins_to_its_frame() {
        let frame = json!({"t": "op_result", "id": 3, "ok": {"text": "ä\"\\\n😀".repeat(500)}});
        let pieces = fit(frame.to_string(), || 4, 200).unwrap();
        assert!(pieces.len() > 3);
        let mut j = Joiner::default();
        assert_eq!(feed(&mut j, &pieces), [Ok(Some(frame.clone()))]);
        assert_eq!(j.open(), None);
        // Two in a row, and a frame between them.
        let mut twice = pieces.clone();
        twice.push(json!({"t": "event", "kind": "x"}).to_string());
        twice.extend(fit(frame.to_string(), || 5, 200).unwrap());
        assert_eq!(
            feed(&mut j, &twice),
            [Ok(Some(frame.clone())), Ok(None), Ok(Some(frame))]
        );
    }

    /// Every way a split goes wrong is an error naming it; the split is dropped, its later pieces
    /// are read past, and the next split joins.
    #[test]
    fn a_bad_split_is_an_error_and_dropped() {
        let frame = json!({"t": "event", "kind": "x", "pad": "y".repeat(300)}).to_string();
        let pieces = fit(frame.clone(), || 1, 100).unwrap();
        let n = pieces.len();
        let next = fit(frame.clone(), || 2, 100).unwrap();
        let errors = |frames: Vec<String>| -> Vec<String> {
            let mut j = Joiner::default();
            let out = feed(&mut j, &frames);
            assert_eq!(
                out.last(),
                Some(&Ok(Some(serde_json::from_str(&frame).unwrap()))),
                "the next split joins after: {out:?}"
            );
            out.into_iter()
                .filter_map(|r| r.err().map(|e| e.message))
                .collect()
        };
        let with_next = |mut frames: Vec<String>| {
            frames.extend(next.clone());
            frames
        };

        // A piece skipped.
        let mut gap = pieces.clone();
        gap.remove(1);
        let e = errors(with_next(gap));
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(
            e[0].contains("split 1's piece 2 came where piece 1 was due"),
            "{e:?}"
        );

        // Another frame between the pieces.
        let mut between = pieces.clone();
        between.insert(1, json!({"t": "event", "kind": "y"}).to_string());
        let e = errors(with_next(between));
        assert_eq!(e.len(), 1, "{e:?}");
        assert!(
            e[0].contains("split 1 was interrupted by the extension's event frame"),
            "{e:?}"
        );

        // Another split's piece between them.
        let mut other = pieces[..2].to_vec();
        other.extend(next.clone());
        let mut j = Joiner::default();
        let out = feed(&mut j, &other);
        let e: Vec<_> = out.iter().filter_map(|r| r.as_ref().err()).collect();
        assert_eq!(e.len(), 1, "{out:?}");
        assert!(
            e[0].message
                .contains("split 1 was interrupted by split 2's chunk"),
            "{e:?}"
        );
        assert_eq!(e[0].split, Some(1));
        assert!(matches!(out.last(), Some(Ok(Some(_)))), "split 2 joins");

        // An end whose count is not the pieces'.
        let mut short = pieces.clone();
        short[n - 1] = json!({"t": "end", "split": 1, "seq": n}).to_string();
        let e = errors(with_next(short));
        assert!(
            e[0].contains(&format!(
                "split 1 ended at seq {n}, and {} pieces came",
                n - 1
            )),
            "{e:?}"
        );

        // A piece of a split that never began, and an end of one.
        let e = errors(with_next(pieces[1..].to_vec()));
        assert_eq!(e.len(), 1, "the rest is read past: {e:?}");
        assert!(
            e[0].contains("piece 1 came, and its piece 0 never did"),
            "{e:?}"
        );
        let e = errors(with_next(vec![
            json!({"t": "end", "split": 9, "seq": 0}).to_string(),
        ]));
        assert!(
            e[0].contains("an end of split 9, which never began"),
            "{e:?}"
        );

        // Pieces without numbers or data.
        let e = errors(with_next(vec![
            json!({"t": "chunk", "seq": 0, "data": "{"}).to_string(),
        ]));
        assert!(e[0].contains("needs a whole-number split and seq"), "{e:?}");
        let e = errors(with_next(vec![
            json!({"t": "chunk", "split": 7, "seq": 0}).to_string(),
        ]));
        assert!(e[0].contains("split 7's piece 0 has no text data"), "{e:?}");

        // Text that is no frame.
        for (text, says) in [
            ("[1, 2]", "no frame"),
            ("{\"t\": \"chunk\"}", "no frame"),
            ("{\"t\": ", "no JSON"),
        ] {
            let e = errors(with_next(vec![
                json!({"t": "chunk", "split": 8, "seq": 0, "data": text}).to_string(),
                json!({"t": "end", "split": 8, "seq": 1}).to_string(),
            ]));
            assert!(
                e[0].contains("split 8 joined to") && e[0].contains(says),
                "{e:?}"
            );
        }
    }

    /// A body's bytes as base64 pieces: each frame within the limit, each `data` decoding on its
    /// own, the pieces joining to the bytes; numbered on from earlier pieces of the same body.
    #[test]
    fn bytes_pieces_decode_on_their_own_and_join() {
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD;
        for (len, limit) in [
            (0usize, 64usize),
            (1, 64),
            (2, 64),
            (3, 64),
            (100, 64),
            (5000, 97),
        ] {
            let bytes: Vec<u8> = (0..len).map(|i| (i * 7 % 256) as u8).collect();
            let mut c = Chunker::new(Key::Id(json!(12)), limit);
            let mut rest = bytes.as_slice();
            let mut joined = Vec::new();
            let mut n = 0;
            while !rest.is_empty() {
                let f = c.bytes_piece(&mut rest).unwrap();
                assert!(f.len() <= limit, "{} > {limit}", f.len());
                let m = read(&f);
                assert_eq!(
                    (m["t"].as_str(), m["id"].as_i64(), m["seq"].as_u64()),
                    (Some("chunk"), Some(12), Some(n))
                );
                joined.extend(b64.decode(m["data"].as_str().unwrap()).unwrap());
                n += 1;
            }
            assert_eq!(c.count(), n);
            assert_eq!(joined, bytes, "{len} bytes within {limit}");
            assert_eq!(read(&c.end())["seq"], n);
        }
        // A key that leaves no room is NoRoom, as for text.
        let c = Chunker::new(Key::Id(json!("x".repeat(80))), 64);
        assert!(c.check().is_err());
    }
}
