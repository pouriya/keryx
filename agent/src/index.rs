//! The search index: tantivy, in `index/` beside `agent.db`, holding memory notes always and
//! messages' plaintext only when `[search] plain` is on. `agent.db` is the truth: the index's last
//! commit names the newest rows it covers, and a missing or stale index is rebuilt from it.
//!
//! The ranking is vendored from borhan 1.1.1 (`/p/test/borhan`, `src/search.rs` and `src/index.rs`,
//! MIT, by the same author), as agent/AGENTS.md asks: borhan's query syntax, `(a b)` one idea spelled
//! several ways, `+` required, `-` excluded, quotes a phrase; max within a clause, coverage across
//! clauses as `(hits/total)^1.5`, minimum-span proximity, recency with a floor, and the words that
//! matched nothing and the hints the top results share. Its comments are kept where the code is.
//!
//! Deviations, each deliberate:
//! - **One document per message or note**, not per paragraph: a keryx message is chat-sized, and a
//!   note is what the model wrote as one thing. So there is no `context` field, no `Source::Context`
//!   tier, no per-message diversity cap and no snippet sentence table: the snippet is the sentence
//!   holding the most clauses, cut from the stored text.
//! - **No fuzzy matching.** borhan's is opt-in per search, and the built-ins take no such option.
//! - **Filters are keryx's:** the kind (message or note), the chat or memory, sender, topic, thread
//!   and a time window, in place of borhan's session, roles and window.
//! - **Edits and deletes per document**, by its key, since a deleted keryx message must leave at once.
//! - **The hints are plain words**, without their counts.

use std::collections::HashMap;
use std::ops::Bound;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use keryx_core::ulid::Ulid;
use tantivy::collector::{Collector, SegmentCollector};
use tantivy::columnar::Column;
use tantivy::directory::{Directory, INDEX_WRITER_LOCK};
use tantivy::postings::{Postings, SegmentPostings};
use tantivy::query::{
    BooleanQuery, EmptyQuery, EnableScoring, Occur, PhrasePrefixQuery, PhraseQuery, Query,
    RangeQuery, Scorer, TermQuery, Weight,
};
use tantivy::query_grammar::{self, Delimiter, UserInputAst, UserInputLeaf, UserInputLiteral};
use tantivy::schema::{
    FAST, INDEXED, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value,
};
use tantivy::{
    DocAddress, DocId, DocSet, IndexReader, IndexWriter, SegmentOrdinal, SegmentReader,
    TantivyDocument, Term, doc,
};

use crate::normalize::{self, Analyzer};
use crate::store::NoteRow;

/// The index directory, beside `agent.db`.
pub const DIRECTORY: &str = "index";

/// The timestamp fast field, looked up by name inside the collector.
const TS: &str = "ts";

/// Bytes of heap the writer may use before it flushes a segment. borhan's.
const HEAP: usize = 50_000_000;

/// How hard coverage is weighted. borhan's.
const ALPHA: f32 = 1.5;

/// Largest multiplier a tight span between two clauses can earn. borhan's.
const PROXIMITY: f32 = 0.5;

/// Half-life of the recency decay, in milliseconds: ninety days. borhan's.
const HALF_LIFE: f32 = 90.0 * 24.0 * 60.0 * 60.0 * 1000.0;

/// Smallest multiplier the recency decay can reach. borhan's.
const FLOOR: f32 = 0.3;

/// Terms returned as hints. borhan's.
const HINTS: usize = 6;

/// Units a term must appear in before it is offered as a hint. borhan's.
const HINT_FLOOR: u64 = 2;

/// Top-level clauses a query may score. borhan's.
const CLAUSES: usize = 32;

/// The share of documents past which a word names the whole index rather than anything in it, for
/// the preamble's hint words. borhan's `CONTEXT_CEILING`.
const CEILING: f64 = 0.15;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the index at {path}: {source}")]
    Tantivy {
        path: PathBuf,
        #[source]
        source: tantivy::TantivyError,
    },
    #[error("the query is empty: pass the words to find, like (error fault) token")]
    Empty,
    #[error(
        "the query {query:?} does not parse: {detail}. Quote a phrase with \"…\", close every \
         parenthesis you open, and put a backslash before any of : ( ) [ ] {{ }} ^ \" ' that is \
         part of a word"
    )]
    Syntax { query: String, detail: String },
    #[error(
        "`{field}:` is not something a query can name: the only fields are surface: (exact \
         spelling) and lemma: (folded form)"
    )]
    Field { field: String },
    #[error(
        "regular expressions (/…/) are not supported: write out the forms you mean inside \
         parentheses, like (rotate rotated rotation)"
    )]
    Regex,
    #[error(
        "ranges ([a TO b], >a, <=b) are not supported: use since= and until= for a time window"
    )]
    Range,
    #[error("`*` and `field:*` match everything and rank nothing: search for words")]
    Everything,
    #[error(
        "{word}* is a one-word prefix, which is not supported: write out the forms you mean inside \
         parentheses, like (rotate rotated rotation), or end a phrase of two or more words with *"
    )]
    Prefix { word: String },
    #[error(
        "every part of the query is excluded with - or NOT: say what to find, then what to leave \
         out, like token -expired"
    )]
    Unwanted,
    #[error(
        "the query has {count} top-level parts and at most 32 are scored: put the words that mean \
         the same thing inside one pair of parentheses"
    )]
    Clauses { count: usize },
}

impl Error {
    /// Whether the query was at fault, rather than the index.
    pub fn is_query(&self) -> bool {
        !matches!(self, Self::Tantivy { .. })
    }
}

/// How a query word reached a document. borhan's, without fuzzy and context.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Source {
    Exact,
    Variant,
}

impl Source {
    fn weight(&self) -> f32 {
        match self {
            Source::Exact => 1.0,
            Source::Variant => 0.9,
        }
    }
}

/// What a document is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    Message = 0,
    Note = 1,
}

/// One document to index: a message's newest text, or a note.
pub struct Document {
    pub kind: DocKind,
    /// A message's chat, or a note's chat memory, none for the global one.
    pub chat: Option<Ulid>,
    /// A message's seq, or none for a note.
    pub seq: Option<u64>,
    /// A note's id, or none for a message.
    pub note: Option<Ulid>,
    pub sender: Option<Ulid>,
    pub topic: Option<Ulid>,
    pub thread: Option<u64>,
    pub at: i64,
    pub text: String,
}

/// What narrows a search before anything is scored.
#[derive(Debug, Clone)]
pub struct Filter {
    pub kind: DocKind,
    /// Messages of these chats only, or notes of this one memory: `Some(None)` is the global one.
    pub chat_list: Option<Vec<Option<Ulid>>>,
    pub sender: Option<Ulid>,
    pub topic: Option<Ulid>,
    pub thread: Option<u64>,
    pub since: Option<i64>,
    pub until: Option<i64>,
}

/// One result.
#[derive(Debug, Clone)]
pub struct Hit {
    pub chat: Option<Ulid>,
    pub seq: Option<u64>,
    pub note: Option<Ulid>,
    pub snippet: String,
    /// Clauses matched over clauses asked for.
    pub coverage: (u8, u8),
}

/// Everything one search returns.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub hit_list: Vec<Hit>,
    pub more: bool,
    pub unknown_list: Vec<String>,
    pub hint_list: Vec<String>,
}

/// What the index's last commit covers, in its payload.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Covered {
    /// The normalizer's version the index was built with.
    pub version: u32,
    /// The newest message's server time it holds.
    pub messages: i64,
    /// The newest note change it holds.
    pub notes: i64,
}

#[derive(Debug, Clone, Copy)]
struct Fields {
    key: tantivy::schema::Field,
    kind: tantivy::schema::Field,
    chat: tantivy::schema::Field,
    seq: tantivy::schema::Field,
    sender: tantivy::schema::Field,
    topic: tantivy::schema::Field,
    thread: tantivy::schema::Field,
    ts: tantivy::schema::Field,
    text: tantivy::schema::Field,
    surface: tantivy::schema::Field,
    lemma: tantivy::schema::Field,
}

/// The one index, open for reading and writing. Its writer holds tantivy's lock for the process.
pub struct Index {
    fields: Fields,
    reader: IndexReader,
    writer: IndexWriter,
    path: PathBuf,
    covered: Covered,
}

/// The key a document is replaced and deleted by.
fn key_of(chat: Option<Ulid>, seq: Option<u64>, note: Option<Ulid>) -> String {
    match note {
        Some(note) => format!("n:{note}"),
        None => format!(
            "m:{}:{}",
            chat.map(|chat| chat.to_string()).unwrap_or_default(),
            seq.unwrap_or(0)
        ),
    }
}

/// A chat as the `chat` field spells it: its id, or `-` for the global memory.
fn chat_text(chat: Option<Ulid>) -> String {
    match chat {
        Some(chat) => chat.to_string(),
        None => "-".to_string(),
    }
}

impl Index {
    /// Opens the index in `directory`, making it when missing. A stale one, built by another
    /// normalizer, is emptied, and `covered()` then says nothing is in it.
    pub fn open(directory: &Path) -> Result<Self, Error> {
        let path = directory.join(DIRECTORY);
        let failed = |source: tantivy::TantivyError| Error::Tantivy {
            path: path.clone(),
            source,
        };
        let mut builder = Schema::builder();
        let body = |tokenizer: &str| {
            TextOptions::default().set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer(tokenizer)
                    .set_index_option(IndexRecordOption::WithFreqsAndPositions),
            )
        };
        let fields = Fields {
            key: builder.add_text_field("key", STRING | STORED),
            kind: builder.add_u64_field("kind", INDEXED),
            chat: builder.add_text_field("chat", STRING | STORED),
            seq: builder.add_u64_field("seq", STORED),
            sender: builder.add_text_field("sender", STRING),
            topic: builder.add_text_field("topic", STRING),
            thread: builder.add_u64_field("thread", INDEXED),
            ts: builder.add_i64_field(TS, INDEXED | FAST),
            text: builder.add_text_field("text", STORED),
            surface: builder.add_text_field(normalize::SURFACE, body(normalize::SURFACE)),
            lemma: builder.add_text_field(normalize::LEMMA, body(normalize::LEMMA)),
        };
        if let Err(source) = std::fs::create_dir_all(&path) {
            return Err(failed(tantivy::TantivyError::IoError(Arc::new(source))));
        }
        let directory = match tantivy::directory::MmapDirectory::open(&path) {
            Ok(directory) => directory,
            Err(source) => return Err(failed(source.into())),
        };
        let schema = builder.build();
        let index = {
            // Only replace derived index metadata while no other agent is writing it.
            let _writer_lock = match directory.acquire_lock(&INDEX_WRITER_LOCK) {
                Ok(lock) => lock,
                Err(error) => return Err(failed(tantivy::TantivyError::LockFailure(error, None))),
            };
            match tantivy::Index::open_or_create(directory.clone(), schema.clone()) {
                Ok(index) => index,
                Err(tantivy::TantivyError::SchemaError(_)) => {
                    match tantivy::Index::create(directory, schema, Default::default()) {
                        Ok(index) => index,
                        Err(error) => return Err(failed(error)),
                    }
                }
                Err(error) => return Err(failed(error)),
            }
        };
        index
            .tokenizers()
            .register(normalize::SURFACE, Analyzer::new(false));
        index
            .tokenizers()
            .register(normalize::LEMMA, Analyzer::new(true));
        let reader = index.reader().map_err(failed)?;
        let writer = index.writer::<TantivyDocument>(HEAP).map_err(failed)?;
        let payload = index.load_metas().map_err(failed)?.payload;
        let mut covered: Covered = payload
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        if covered.version != normalize::VERSION {
            writer.delete_all_documents().map_err(failed)?;
            covered = Covered {
                version: normalize::VERSION,
                messages: 0,
                notes: 0,
            };
        }
        let mut opened = Self {
            fields,
            reader,
            writer,
            path,
            covered,
        };
        opened.commit(covered)?;
        Ok(opened)
    }

    fn failed(&self, source: tantivy::TantivyError) -> Error {
        Error::Tantivy {
            path: self.path.clone(),
            source,
        }
    }

    /// What the last commit covers.
    pub fn covered(&self) -> Covered {
        self.covered
    }

    /// Adds a document, replacing any with its key. Seen by searches after the next commit.
    pub fn put(&mut self, document: &Document) -> Result<(), Error> {
        let key = key_of(document.chat, document.seq, document.note);
        self.writer
            .delete_term(Term::from_field_text(self.fields.key, &key));
        let fields = self.fields;
        let added = doc!(
            fields.key => key,
            fields.kind => document.kind as u64,
            fields.chat => chat_text(document.chat),
            fields.seq => document.seq.unwrap_or(0),
            fields.sender => document.sender.map(|sender| sender.to_string()).unwrap_or_default(),
            fields.topic => document.topic.map(|topic| topic.to_string()).unwrap_or_default(),
            fields.thread => document.thread.unwrap_or(0),
            fields.ts => document.at,
            fields.text => document.text.clone(),
            fields.surface => document.text.clone(),
            fields.lemma => document.text.clone(),
        );
        if let Err(source) = self.writer.add_document(added) {
            return Err(self.failed(source));
        }
        Ok(())
    }

    /// Indexes a note from `agent.db`, whether just saved or read during a rebuild.
    pub fn put_note(&mut self, note: NoteRow) -> Result<(), Error> {
        self.put(&Document {
            kind: DocKind::Note,
            chat: note.chat,
            seq: None,
            note: Some(note.id),
            sender: None,
            topic: None,
            thread: None,
            at: note.updated_at,
            text: note.text,
        })
    }

    /// Deletes a message's or a note's document.
    pub fn delete(&mut self, chat: Option<Ulid>, seq: Option<u64>, note: Option<Ulid>) {
        let key = key_of(chat, seq, note);
        self.writer
            .delete_term(Term::from_field_text(self.fields.key, &key));
    }

    /// Deletes every message, for turning `[search] plain` off.
    pub fn delete_messages(&mut self) {
        self.writer.delete_term(Term::from_field_u64(
            self.fields.kind,
            DocKind::Message as u64,
        ));
    }

    /// Commits what was added and deleted, naming what it now covers, and shows it to searches.
    pub fn commit(&mut self, covered: Covered) -> Result<(), Error> {
        let payload = serde_json::to_string(&covered).unwrap_or_default();
        let mut prepared = match self.writer.prepare_commit() {
            Ok(prepared) => prepared,
            Err(source) => return Err(self.failed(source)),
        };
        prepared.set_payload(&payload);
        if let Err(source) = prepared.commit() {
            return Err(self.failed(source));
        }
        if let Err(source) = self.reader.reload() {
            return Err(self.failed(source));
        }
        self.covered = covered;
        Ok(())
    }

    /// The bytes the index takes on disk.
    pub fn size(&self) -> u64 {
        directory_size(&self.path)
    }

    /// The most common words of one memory's notes, as the normalizer folds them, leaving out those
    /// in more than `CEILING` of them, which name the memory rather than anything in it, as borhan's
    /// lexicon does. `texts` are the memory's notes.
    pub fn common_words(texts: &[String], limit: usize) -> Vec<String> {
        let mut counts: HashMap<String, u64> = HashMap::new();
        for text in texts {
            let mut seen: Vec<String> = Vec::new();
            for word in normalize::segment(text) {
                let lemma = normalize::lemma(word.text, word.script);
                if lemma.is_empty() || seen.contains(&lemma) {
                    continue;
                }
                seen.push(lemma.clone());
                *counts.entry(lemma).or_insert(0) += 1;
            }
        }
        let total = texts.len() as f64;
        let ceiling = (total * CEILING).ceil() as u64;
        let mut words: Vec<(String, u64)> = Vec::new();
        for (word, count) in counts {
            if texts.len() >= 7 && count > ceiling {
                continue;
            }
            words.push((word, count));
        }
        words.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
        words.truncate(limit);
        let mut list = Vec::new();
        for (word, _) in words {
            list.push(word);
        }
        list
    }

    /// Runs a search: hits `skip` to `skip + limit` by rank, with whether more follow.
    pub fn search(
        &self,
        query: &str,
        filter: &Filter,
        limit: usize,
        skip: usize,
    ) -> Result<Outcome, Error> {
        let text = query.trim();
        if text.is_empty() {
            return Err(Error::Empty);
        }
        let ast = match query_grammar::parse_query(text) {
            Ok(ast) => ast,
            Err(_) => {
                let (_, errors) = query_grammar::parse_query_lenient(text);
                let mut detail = Vec::new();
                for error in &errors {
                    let mut at = error.pos;
                    if at <= text.len() && text.is_char_boundary(at) {
                        at = text[..at].chars().count();
                    }
                    detail.push(format!("{} at character {at}", error.message));
                }
                if detail.is_empty() {
                    detail.push("it is not a query this search can read".to_string());
                }
                return Err(Error::Syntax {
                    query: text.to_string(),
                    detail: detail.join("; "),
                });
            }
        };

        // Whether one pair of parentheses holds the whole query, as borhan reads it.
        let bare = match text.strip_prefix('+') {
            Some(rest) => rest.trim_start(),
            None => text,
        };
        let mut whole = false;
        if bare.starts_with('(') {
            let mut depth = 0usize;
            let mut quote: Option<char> = None;
            let mut escaped = false;
            let mut close = None;
            for (at, character) in bare.char_indices() {
                if escaped {
                    escaped = false;
                    continue;
                }
                if character == '\\' {
                    escaped = true;
                    continue;
                }
                if let Some(open) = quote {
                    if character == open {
                        quote = None;
                    }
                    continue;
                }
                match character {
                    '"' | '\'' => quote = Some(character),
                    '(' => depth += 1,
                    ')' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            close = Some(at);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            if let Some(close) = close {
                let rest = bare[close + 1..].trim();
                whole = match rest.strip_prefix('^') {
                    Some(boost) => boost.parse::<f64>().is_ok(),
                    None => rest.is_empty(),
                };
            }
        }
        let mut parts: Vec<(Occur, &UserInputAst)> = Vec::new();
        match &ast {
            UserInputAst::Clause(children) if !whole => {
                for (occur, child) in children {
                    let occur = match occur {
                        Some(occur) => *occur,
                        None => Occur::Should,
                    };
                    parts.push((occur, child));
                }
            }
            _ => parts.push((Occur::Should, &ast)),
        }

        let searcher = self.reader.searcher();
        let fields = self.fields;
        let mut state = Compiling {
            searcher: &searcher,
            fields,
            unknown: Vec::new(),
            texts: Vec::new(),
            probes: 0,
        };
        let mut top: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        let mut concepts: Vec<Concept> = Vec::new();
        let mut wanted = false;
        for (occur, part) in parts {
            let excluded = occur == Occur::MustNot;
            state.texts = Vec::new();
            state.probes = 0;
            let (query, node) = compile(part, excluded, &mut state)?;
            top.push((occur, query));
            if excluded {
                continue;
            }
            wanted = true;
            if state.probes == 0 {
                continue;
            }
            concepts.push(Concept {
                node,
                texts: std::mem::take(&mut state.texts),
            });
        }
        if !wanted {
            return Err(Error::Unwanted);
        }
        if concepts.len() > CLAUSES {
            return Err(Error::Clauses {
                count: concepts.len(),
            });
        }
        let unknown_list = state.unknown;
        if concepts.is_empty() {
            return Ok(Outcome {
                hit_list: Vec::new(),
                more: false,
                unknown_list,
                hint_list: Vec::new(),
            });
        }

        let raw_text = |field: tantivy::schema::Field, text: &str| -> Box<dyn Query> {
            Box::new(TermQuery::new(
                Term::from_field_text(field, text),
                IndexRecordOption::Basic,
            ))
        };
        let mut clauses: Vec<(Occur, Box<dyn Query>)> =
            vec![(Occur::Must, Box::new(BooleanQuery::new(top)))];
        clauses.push((
            Occur::Must,
            Box::new(TermQuery::new(
                Term::from_field_u64(fields.kind, filter.kind as u64),
                IndexRecordOption::Basic,
            )),
        ));
        if let Some(chat_list) = &filter.chat_list {
            let mut chats: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            for chat in chat_list {
                chats.push((Occur::Should, raw_text(fields.chat, &chat_text(*chat))));
            }
            if chats.is_empty() {
                chats.push((Occur::Should, Box::new(EmptyQuery)));
            }
            clauses.push((Occur::Must, Box::new(BooleanQuery::new(chats))));
        }
        if let Some(sender) = filter.sender {
            clauses.push((Occur::Must, raw_text(fields.sender, &sender.to_string())));
        }
        if let Some(topic) = filter.topic {
            clauses.push((Occur::Must, raw_text(fields.topic, &topic.to_string())));
        }
        if let Some(thread) = filter.thread {
            clauses.push((
                Occur::Must,
                Box::new(TermQuery::new(
                    Term::from_field_u64(fields.thread, thread),
                    IndexRecordOption::Basic,
                )),
            ));
        }
        if filter.since.is_some() || filter.until.is_some() {
            let lower = match filter.since {
                Some(since) => Bound::Included(Term::from_field_i64(fields.ts, since)),
                None => Bound::Unbounded,
            };
            let upper = match filter.until {
                Some(until) => Bound::Excluded(Term::from_field_i64(fields.ts, until)),
                None => Bound::Unbounded,
            };
            clauses.push((Occur::Must, Box::new(RangeQuery::new(lower, upper))));
        }
        let query = BooleanQuery::new(clauses);

        let ranking = Ranking {
            concepts: Arc::new(concepts),
            now: Ulid::now(),
            limit: skip + limit + 1,
        };
        let candidates = match searcher.search(&query, &ranking) {
            Ok(candidates) => candidates,
            Err(source) => return Err(self.failed(source)),
        };
        let more = candidates.len() > skip + limit;
        let mut hit_list = Vec::new();
        let mut texts: Vec<String> = Vec::new();
        for candidate in candidates.iter().skip(skip).take(limit) {
            let document: TantivyDocument = match searcher.doc(candidate.address) {
                Ok(document) => document,
                Err(source) => return Err(self.failed(source)),
            };
            let mut chat = None;
            let mut seq = None;
            let mut note = None;
            let mut body = String::new();
            let mut key = String::new();
            for (field, value) in document.field_values() {
                if field == fields.text {
                    body = value.as_str().unwrap_or_default().to_string();
                } else if field == fields.chat {
                    chat = value.as_str().and_then(|text| Ulid::parse(text).ok());
                } else if field == fields.seq {
                    seq = value.as_u64().filter(|seq| *seq > 0);
                } else if field == fields.key {
                    key = value.as_str().unwrap_or_default().to_string();
                }
            }
            if let Some(id) = key.strip_prefix("n:") {
                note = Ulid::parse(id).ok();
                seq = None;
            }
            hit_list.push(Hit {
                chat,
                seq,
                note,
                snippet: snippet(&body, &ranking.concepts),
                coverage: (candidate.hits, ranking.concepts.len() as u8),
            });
            texts.push(body);
        }
        let hint_list = hints(&searcher, fields, &texts, &ranking.concepts);
        Ok(Outcome {
            hit_list,
            more,
            unknown_list,
            hint_list,
        })
    }
}

/// The bytes of every file under `path`.
pub fn directory_size(path: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.is_dir() {
            total += directory_size(&entry.path());
        } else {
            total += metadata.len();
        }
    }
    total
}

/// One way a piece of the query can match a document: a query on one field, at one tier.
struct Probe {
    weight: Box<dyn Weight>,
    source: Source,
    positions: Vec<Term>,
}

/// A piece of the query, shaped the way it is scored.
enum Node {
    Leaf(Vec<Probe>),
    Clause(Vec<(Occur, Node)>),
    Boost(Box<Node>, f32),
}

/// One top-level part of the query: what coverage counts.
struct Concept {
    node: Node,
    texts: Vec<String>,
}

/// What compiling the query carries from one piece to the next.
struct Compiling<'a> {
    searcher: &'a tantivy::Searcher,
    fields: Fields,
    unknown: Vec<String>,
    texts: Vec<String>,
    probes: usize,
}

/// Turns one piece of the parsed query into the query that finds candidates and the node that
/// scores them. borhan's, without fuzzy and context.
fn compile(
    ast: &UserInputAst,
    negated: bool,
    state: &mut Compiling,
) -> Result<(Box<dyn Query>, Node), Error> {
    let literal = match ast {
        UserInputAst::Clause(children) => {
            let mut queries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
            let mut nodes = Vec::new();
            for (occur, child) in children {
                let occur = match occur {
                    Some(occur) => *occur,
                    None => Occur::Should,
                };
                let excluded = occur == Occur::MustNot;
                let (query, node) = compile(child, negated || excluded, state)?;
                queries.push((occur, query));
                if !excluded {
                    nodes.push((occur, node));
                }
            }
            return Ok((Box::new(BooleanQuery::new(queries)), Node::Clause(nodes)));
        }
        UserInputAst::Boost(inner, boost) => {
            let (query, node) = compile(inner, negated, state)?;
            return Ok((query, Node::Boost(Box::new(node), boost.0 as f32)));
        }
        UserInputAst::Leaf(leaf) => match leaf.as_ref() {
            UserInputLeaf::Literal(literal) => literal,
            UserInputLeaf::Regex { .. } => return Err(Error::Regex),
            UserInputLeaf::Range { .. } => return Err(Error::Range),
            UserInputLeaf::All | UserInputLeaf::Exists { .. } => return Err(Error::Everything),
            UserInputLeaf::Set { field, elements } => {
                let mut children = Vec::new();
                for element in elements {
                    let member = UserInputLiteral {
                        field_name: field.clone(),
                        phrase: element.clone(),
                        delimiter: Delimiter::None,
                        slop: 0,
                        prefix: false,
                    };
                    children.push((
                        Some(Occur::Should),
                        UserInputAst::from(UserInputLeaf::Literal(member)),
                    ));
                }
                return compile(&UserInputAst::Clause(children), negated, state);
            }
        },
    };

    let all = state.fields;
    let targets = match literal.field_name.as_deref() {
        None => vec![(all.surface, Source::Exact), (all.lemma, Source::Variant)],
        Some(normalize::SURFACE) => vec![(all.surface, Source::Exact)],
        Some(normalize::LEMMA) => vec![(all.lemma, Source::Variant)],
        Some(field) => {
            return Err(Error::Field {
                field: field.to_string(),
            });
        }
    };

    let mut words: Vec<(usize, String, String, String)> = Vec::new();
    for (position, word) in normalize::segment(&literal.phrase).into_iter().enumerate() {
        words.push((
            position,
            word.text.to_string(),
            normalize::surface(word.text),
            normalize::lemma(word.text, word.script),
        ));
    }
    let starred = literal.delimiter == Delimiter::None && literal.phrase.ends_with('*');
    if starred || (literal.prefix && words.len() < 2) {
        return Err(Error::Prefix {
            word: literal.phrase.trim_end_matches('*').to_string(),
        });
    }

    let mut found = vec![false; words.len()];
    let mut queries: Vec<(Occur, Box<dyn Query>)> = Vec::new();
    let mut probes = Vec::new();
    for (field, source) in targets {
        let mut terms: Vec<(usize, Term)> = Vec::new();
        let mut absent = false;
        for (at, (position, _, surface, lemma)) in words.iter().enumerate() {
            let text = if field == all.surface { surface } else { lemma };
            if text.is_empty() {
                continue;
            }
            let term = Term::from_field_text(field, text);
            if !(literal.prefix && at + 1 == words.len()) {
                let frequency = match state.searcher.doc_freq(&term) {
                    Ok(frequency) => frequency,
                    Err(source) => {
                        return Err(Error::Tantivy {
                            path: PathBuf::new(),
                            source,
                        });
                    }
                };
                if frequency == 0 {
                    absent = true;
                } else {
                    found[at] = true;
                }
            }
            terms.push((*position, term));
        }
        if terms.is_empty() || absent {
            continue;
        }
        let mut positions = Vec::new();
        let query: Box<dyn Query> = if terms.len() == 1 {
            if literal.prefix {
                continue;
            }
            positions.push(terms[0].1.clone());
            Box::new(TermQuery::new(
                terms[0].1.clone(),
                IndexRecordOption::WithFreqs,
            ))
        } else if literal.prefix {
            for (_, term) in &terms[..terms.len() - 1] {
                positions.push(term.clone());
            }
            Box::new(PhrasePrefixQuery::new_with_offset(terms))
        } else {
            for (_, term) in &terms {
                positions.push(term.clone());
            }
            Box::new(PhraseQuery::new_with_offset_and_slop(terms, literal.slop))
        };
        let weight = match query.weight(EnableScoring::enabled_from_searcher(state.searcher)) {
            Ok(weight) => weight,
            Err(source) => {
                return Err(Error::Tantivy {
                    path: PathBuf::new(),
                    source,
                });
            }
        };
        queries.push((Occur::Should, query));
        probes.push(Probe {
            weight,
            source,
            positions,
        });
    }

    if !negated {
        for (at, (_, text, surface, lemma)) in words.iter().enumerate() {
            for held in [surface, lemma] {
                if !held.is_empty() && !state.texts.contains(held) {
                    state.texts.push(held.clone());
                }
            }
            if found[at] || (literal.prefix && at + 1 == words.len()) {
                continue;
            }
            if !state.unknown.contains(text) {
                state.unknown.push(text.clone());
            }
        }
        state.probes += probes.len();
    }

    let query: Box<dyn Query> = if queries.is_empty() {
        Box::new(EmptyQuery)
    } else {
        Box::new(BooleanQuery::new(queries))
    };
    Ok((query, Node::Leaf(probes)))
}

/// The best sentence of a document, or the whole of it when it is short: the one holding the most
/// distinct clauses, as borhan picks it.
fn snippet(body: &str, concepts: &[Concept]) -> String {
    let mut sentence_list = Vec::new();
    let mut start = 0;
    for (at, character) in body.char_indices() {
        if matches!(character, '.' | '!' | '?' | '\n' | '؟' | '。') {
            let end = at + character.len_utf8();
            if !body[start..end].trim().is_empty() {
                sentence_list.push((start, end));
            }
            start = end;
        }
    }
    if !body[start..].trim().is_empty() {
        sentence_list.push((start, body.len()));
    }
    if sentence_list.len() < 2 {
        return body.trim().to_string();
    }
    let mut best = (0usize, sentence_list[0].0, sentence_list[0].1);
    for (start, end) in sentence_list {
        let text = &body[start..end];
        let mut hits = 0;
        for concept in concepts {
            let mut found = false;
            for word in normalize::segment(text) {
                let surface = normalize::surface(word.text);
                let lemma = normalize::lemma(word.text, word.script);
                if concept
                    .texts
                    .iter()
                    .any(|held| *held == surface || *held == lemma)
                {
                    found = true;
                    break;
                }
            }
            if found {
                hits += 1;
            }
        }
        if hits > best.0 {
            best = (hits, start, end);
        }
    }
    body[best.1..best.2].trim().to_string()
}

/// The highest-IDF terms across the top results that the caller did not ask for. borhan's.
fn hints(
    searcher: &tantivy::Searcher,
    fields: Fields,
    texts: &[String],
    concepts: &[Concept],
) -> Vec<String> {
    let mut frequencies: HashMap<String, u64> = HashMap::new();
    for text in texts {
        for word in normalize::segment(text) {
            let lemma = normalize::lemma(word.text, word.script);
            if lemma.is_empty() || frequencies.contains_key(&lemma) {
                continue;
            }
            let mut asked = false;
            for concept in concepts {
                if concept.texts.contains(&lemma) {
                    asked = true;
                    break;
                }
            }
            if asked {
                continue;
            }
            let term = Term::from_field_text(fields.lemma, &lemma);
            let frequency = searcher.doc_freq(&term).unwrap_or(0);
            if frequency < HINT_FLOOR {
                continue;
            }
            frequencies.insert(lemma, frequency);
        }
    }
    let mut hints: Vec<(String, u64)> = frequencies.into_iter().collect();
    hints.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    hints.truncate(HINTS);
    let mut list = Vec::new();
    for (word, _) in hints {
        list.push(word);
    }
    list
}

/// One document that survived scoring.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    address: DocAddress,
    score: f32,
    hits: u8,
}

/// The collector that does the actual scoring. borhan's.
struct Ranking {
    concepts: Arc<Vec<Concept>>,
    now: i64,
    limit: usize,
}

impl Collector for Ranking {
    type Fruit = Vec<Candidate>;
    type Child = Scoring;

    fn for_segment(
        &self,
        ordinal: SegmentOrdinal,
        reader: &SegmentReader,
    ) -> tantivy::Result<Scoring> {
        let mut concepts = Vec::with_capacity(self.concepts.len());
        for concept in self.concepts.iter() {
            concepts.push(open(&concept.node, reader)?);
        }
        Ok(Scoring {
            ordinal,
            concepts,
            ts: reader.fast_fields().i64(TS)?,
            now: self.now,
            limit: self.limit,
            best: Vec::with_capacity(self.limit * 2),
            buffer: Vec::new(),
        })
    }

    fn requires_scoring(&self) -> bool {
        false
    }

    fn merge_fruits(&self, fruits: Vec<Vec<Candidate>>) -> tantivy::Result<Vec<Candidate>> {
        let mut merged = Vec::new();
        for fruit in fruits {
            merged.extend(fruit);
        }
        merged.sort_by(|a, b| b.score.total_cmp(&a.score));
        merged.truncate(self.limit);
        Ok(merged)
    }
}

/// A [`Node`] opened on one segment.
enum Live {
    Leaf(Vec<Reading>),
    Clause(Vec<(Occur, Live)>),
    Boost(Box<Live>, f32),
}

/// One [`Probe`] opened on one segment.
struct Reading {
    scorer: Box<dyn Scorer>,
    source: Source,
    positions: Vec<SegmentPostings>,
}

fn open(node: &Node, reader: &SegmentReader) -> tantivy::Result<Live> {
    match node {
        Node::Leaf(probes) => {
            let mut readings = Vec::with_capacity(probes.len());
            for probe in probes {
                let mut positions = Vec::new();
                for term in &probe.positions {
                    let inverted = reader.inverted_index(term.field())?;
                    if let Some(postings) =
                        inverted.read_postings(term, IndexRecordOption::WithFreqsAndPositions)?
                    {
                        positions.push(postings);
                    }
                }
                readings.push(Reading {
                    scorer: probe.weight.scorer(reader, 1.0)?,
                    source: probe.source,
                    positions,
                });
            }
            Ok(Live::Leaf(readings))
        }
        Node::Clause(children) => {
            let mut opened = Vec::with_capacity(children.len());
            for (occur, child) in children {
                opened.push((*occur, open(child, reader)?));
            }
            Ok(Live::Clause(opened))
        }
        Node::Boost(inner, boost) => Ok(Live::Boost(Box::new(open(inner, reader)?), *boost)),
    }
}

/// Scores one document against one piece of the query: a leaf is its best probe, a clause the sum
/// of what it requires plus the best of what it allows, a boost multiplies. borhan's.
fn evaluate(live: &mut Live, doc: DocId, buffer: &mut Vec<u32>, positions: &mut Vec<u32>) -> f32 {
    match live {
        Live::Leaf(readings) => {
            let mut best = 0.0f32;
            for reading in readings.iter_mut() {
                if reading.scorer.doc() < doc {
                    reading.scorer.seek(doc);
                }
                if reading.scorer.doc() != doc {
                    continue;
                }
                let score = reading.scorer.score() * reading.source.weight();
                if score > best {
                    best = score;
                }
                for postings in reading.positions.iter_mut() {
                    if postings.doc() < doc {
                        postings.seek(doc);
                    }
                    if postings.doc() != doc {
                        continue;
                    }
                    buffer.clear();
                    postings.positions(buffer);
                    positions.extend_from_slice(buffer);
                }
            }
            best
        }
        Live::Clause(children) => {
            let mut required = 0.0f32;
            let mut allowed = 0.0f32;
            for (occur, child) in children.iter_mut() {
                let score = evaluate(child, doc, buffer, positions);
                if score <= 0.0 {
                    continue;
                }
                if *occur == Occur::Must {
                    required += score;
                } else if score > allowed {
                    allowed = score;
                }
            }
            required + allowed
        }
        Live::Boost(inner, boost) => evaluate(inner, doc, buffer, positions) * *boost,
    }
}

/// The per-segment half of [`Ranking`].
struct Scoring {
    ordinal: SegmentOrdinal,
    concepts: Vec<Live>,
    ts: Column<i64>,
    now: i64,
    limit: usize,
    best: Vec<Candidate>,
    buffer: Vec<u32>,
}

impl SegmentCollector for Scoring {
    type Fruit = Vec<Candidate>;

    fn collect(&mut self, doc: DocId, _score: tantivy::Score) {
        let total = self.concepts.len();
        let mut sum = 0.0;
        let mut hits = 0u8;
        let mut spans: Vec<Vec<u32>> = Vec::new();
        for concept in self.concepts.iter_mut() {
            let mut positions: Vec<u32> = Vec::new();
            let score = evaluate(concept, doc, &mut self.buffer, &mut positions);
            if score <= 0.0 {
                continue;
            }
            sum += score;
            hits += 1;
            if !positions.is_empty() {
                positions.sort_unstable();
                spans.push(positions);
            }
        }
        if hits == 0 {
            return;
        }
        let coverage = (hits as f32 / total as f32).powf(ALPHA);
        let score = sum * coverage * proximity(&spans) * self.recency(doc);
        self.best.push(Candidate {
            address: DocAddress::new(self.ordinal, doc),
            score,
            hits,
        });
        if self.best.len() >= self.limit * 2 {
            self.best.sort_by(|a, b| b.score.total_cmp(&a.score));
            self.best.truncate(self.limit);
        }
    }

    fn harvest(mut self) -> Vec<Candidate> {
        self.best.sort_by(|a, b| b.score.total_cmp(&a.score));
        self.best.truncate(self.limit);
        self.best
    }
}

impl Scoring {
    /// Exponential decay on age with a floor. borhan's.
    fn recency(&self, doc: DocId) -> f32 {
        let ts = self.ts.first(doc).unwrap_or(self.now);
        let age = (self.now - ts).max(0) as f32;
        FLOOR + (1.0 - FLOOR) * 0.5f32.powf(age / HALF_LIFE)
    }
}

/// The multiplicative proximity bonus. borhan's.
fn proximity(spans: &[Vec<u32>]) -> f32 {
    if spans.len() < 2 {
        return 1.0;
    }
    let mut heads = vec![0usize; spans.len()];
    let mut narrowest = u32::MAX;
    loop {
        let mut low = u32::MAX;
        let mut high = 0u32;
        let mut at = 0;
        for (group, head) in heads.iter().enumerate() {
            let Some(position) = spans[group].get(*head) else {
                return bonus(narrowest, spans.len());
            };
            if *position < low {
                low = *position;
                at = group;
            }
            if *position > high {
                high = *position;
            }
        }
        narrowest = narrowest.min(high - low);
        heads[at] += 1;
    }
}

/// Turns a span into a multiplier. borhan's.
fn bonus(span: u32, clauses: usize) -> f32 {
    if span == u32::MAX {
        return 1.0;
    }
    let tightest = (clauses - 1) as f32;
    let excess = (span as f32 - tightest).max(0.0);
    1.0 + PROXIMITY / (1.0 + excess)
}
