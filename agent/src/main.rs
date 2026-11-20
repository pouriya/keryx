//! The `keryx-agent` binary: an LLM agent that lives in keryx as its own account. `check` reads
//! `agent.toml` and says what is wrong.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Context as _;
use clap::{CommandFactory, Parser, Subcommand};
use keryx_agent::connect::{Driver, Shared, View};
use keryx_agent::index::Index;
use keryx_agent::run::Settings;
use keryx_agent::script::{self, Env};
use keryx_agent::store::Store;
use keryx_core::ulid::Ulid;
use tanzim::validator::{ByteSize, DynamicMap, List, StaticMap};
use tokio::sync::{Notify, mpsc};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::{self, format::FmtSpan};

/// An LLM agent that lives in keryx as its own account, with Starlark as its one tool.
///
/// Every command needs --config.
#[derive(Parser)]
#[command(name = "keryx-agent", version)]
struct CommandLine {
    /// The configuration file, agent.toml. Required on every command, with no default.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,
    /// Log nothing.
    #[arg(long, global = true)]
    quiet: bool,
    /// Log at INFO.
    #[arg(long, global = true)]
    info: bool,
    /// Log at DEBUG, with each line's target.
    #[arg(long, global = true)]
    debug: bool,
    /// Log at TRACE, with each line's target, file and line.
    #[arg(long, global = true)]
    trace: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Start the agent: connect, keep the connection, and run the loop until stopped.
    Run,
    /// Read agent.toml, every wake script and the skills, say what is wrong, and exit. Nothing is
    /// started or written.
    Check,
}

/// The values `[[wake]]`'s lists may hold.
const CHAT_TYPE_LIST: [&str; 4] = ["self", "direct", "group", "channel"];
const CONTENT_KIND_LIST: [&str; 16] = [
    "text",
    "voice",
    "reaction",
    "image",
    "music",
    "video",
    "doc",
    "file",
    "call",
    "media_bundle",
    "question",
    "answer",
    "sticker",
    "suggestion",
    "ack",
    "activity",
];
const EVENT_LIST: [&str; 10] = [
    "message",
    "reaction",
    "answer",
    "ack",
    "added",
    "removed",
    "invitation",
    "join_request",
    "edit",
    "delete",
];

/// Reads `agent.toml` and checks it whole, so every problem is listed at once. Flattening tanzim's
/// error with `{:#}` is the point: that form carries the source, line, column and caret.
fn read_configuration(path: &Path, check_skills: bool) -> anyhow::Result<Settings> {
    let duration = || tanzim::validator::Duration::new();
    let prompt = || {
        StaticMap::new()
            .optional_any("system")
            .optional_any("compact")
            .optional_any("limit")
            .optional_any("send_final")
    };
    let context = || {
        StaticMap::new()
            .optional("max", ByteSize::new())
            .optional_any("around")
            .optional("around_bytes", ByteSize::new())
    };
    let context_budget = || {
        StaticMap::new()
            .optional_any("count")
            .optional("bytes", ByteSize::new())
    };
    let wake = StaticMap::new()
        .optional("chat", List::new().min_len(1))
        .optional("chat_type", List::new().min_len(1))
        .optional("folder", List::new().min_len(1))
        .optional("topic", List::new().min_len(1))
        .optional_any("thread")
        .optional("sender", List::new().min_len(1))
        .optional("content_kind", List::new().min_len(1))
        .optional("event", List::new().min_len(1))
        .optional_any("mentions_me")
        .optional_any("replies_to_me")
        .optional_any("script")
        .optional_any("report_failure")
        .required_any("action")
        .optional("prompt", prompt())
        .optional("context", context());
    let schema = StaticMap::new()
        .required_any("url")
        .required_any("token")
        .required_any("words")
        .required_any("directory")
        .optional_any("timezone")
        .required(
            "llm",
            StaticMap::new()
                .optional_any("protocol")
                .required_any("url")
                .required_any("key")
                .required_any("model")
                .optional("backoff_max", duration()),
        )
        .required("prompt", prompt().required_any("system"))
        .optional("context", context())
        .optional(
            "context_limits",
            StaticMap::new()
                .optional("schedule", context_budget())
                .optional("wake_script", context_budget()),
        )
        .optional("search", StaticMap::new().optional_any("plain"))
        .optional("memory", StaticMap::new().optional_any("hint_words"))
        .optional(
            "skills",
            StaticMap::new()
                .required_any("directory")
                .optional_any("writable"),
        )
        .optional(
            "limits",
            StaticMap::new()
                .optional("script_time", duration())
                .optional("script_output", ByteSize::new())
                .optional_any("tool_calls")
                .optional("wake_script", duration())
                .optional("http_time", duration())
                .optional("http_body", ByteSize::new())
                .optional_any("memory_notes")
                .optional("memory_note", ByteSize::new())
                .optional("storage", ByteSize::new())
                .optional_any("storage_warn")
                .optional("runs_keep", duration()),
        )
        .optional("http", StaticMap::new().optional("allow", List::new()))
        .optional(
            "secrets",
            DynamicMap::new().values(
                StaticMap::new()
                    .required_any("value")
                    .required("hosts", List::new().min_len(1)),
            ),
        )
        .optional("wake", List::new().items(wake));
    let settings: Settings = match tanzim::Config::default()
        .with_source(tanzim::source::file(path.to_string_lossy()))
        .with_schema(schema)
        .try_deserialize()
    {
        Ok(settings) => settings,
        Err(error) => anyhow::bail!("Could not read the configuration: {error:#}"),
    };

    let mut problem_list = Vec::new();
    for (key, url) in [("url", &settings.url), ("llm.url", &settings.llm.url)] {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            problem_list.push(format!("{key} must start with http:// or https://"));
        }
    }
    if settings.llm.protocol != "chat_completions" {
        problem_list.push(format!(
            "llm.protocol is {:?}, and this build only has \"chat_completions\"",
            settings.llm.protocol
        ));
    }
    if let Err(error) = jiff::tz::TimeZone::get(&settings.timezone) {
        problem_list.push(format!(
            "timezone {:?} is not a known IANA zone: {error}",
            settings.timezone
        ));
    }
    if !(settings.limits.storage_warn > 0.0 && settings.limits.storage_warn <= 1.0) {
        problem_list.push(format!(
            "limits.storage_warn is {}, and must be above 0 and at most 1",
            settings.limits.storage_warn
        ));
    }
    let mut path_list = vec![("directory".to_string(), &settings.directory)];
    if let Some(skills) = &settings.skills {
        path_list.push(("skills.directory".to_string(), &skills.directory));
    }
    for (index, rule) in settings.wake.iter().enumerate() {
        let name = format!("wake #{}", index + 1);
        if let Some(script) = &rule.script {
            path_list.push((format!("{name} script"), script));
        }
        if rule.action != "wake" && rule.action != "ignore" {
            problem_list.push(format!(
                "{name} action is {:?}, and must be \"wake\" or \"ignore\"",
                rule.action
            ));
        }
        let known_list: [(&str, &Vec<String>, &[&str]); 3] = [
            ("chat_type", &rule.chat_type, &CHAT_TYPE_LIST),
            ("content_kind", &rule.content_kind, &CONTENT_KIND_LIST),
            ("event", &rule.event, &EVENT_LIST),
        ];
        for (key, value_list, allowed) in known_list {
            for value in value_list {
                if !allowed.contains(&value.as_str()) {
                    problem_list.push(format!("{name} {key} has an unknown {value:?}"));
                }
            }
        }
        for (key, value_list) in [("chat", &rule.chat), ("topic", &rule.topic)] {
            for value in value_list {
                if Ulid::parse(value).is_err() {
                    problem_list.push(format!("{name} {key} has {value:?}, which is not an id"));
                }
            }
        }
    }
    for (key, value) in path_list {
        if !value.is_absolute() {
            problem_list.push(format!("{key} must be an absolute path, and is {value:?}"));
        }
    }
    for (index, rule) in settings.wake.iter().enumerate() {
        let Some(script) = &rule.script else {
            continue;
        };
        match std::fs::read_to_string(script) {
            Ok(text) => {
                if let Err(error) = script::check_wake(&text) {
                    problem_list.push(format!(
                        "wake #{} script {}: {error}",
                        index + 1,
                        script.display()
                    ));
                }
            }
            Err(error) => problem_list.push(format!(
                "wake #{} script {} could not be read: {error}",
                index + 1,
                script.display()
            )),
        }
    }
    if let Some(skills) = &settings.skills {
        match std::fs::read_dir(&skills.directory) {
            Ok(entries) if check_skills => {
                let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
                paths.sort();
                for path in paths {
                    if path.extension().and_then(|extension| extension.to_str()) != Some("md") {
                        continue;
                    }
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    if script::skill_of(&text).is_none() {
                        problem_list.push(format!(
                            "skill {} has no valid frontmatter with name and description",
                            path.display()
                        ));
                    }
                }
            }
            Ok(_) => {
                script::skills_in(&skills.directory);
            }
            Err(error) => problem_list.push(format!(
                "skills.directory {} could not be read: {error}",
                skills.directory.display()
            )),
        }
    }
    if !problem_list.is_empty() {
        anyhow::bail!(
            "{} is not fine:\n{}",
            path.display(),
            problem_list.join("\n")
        );
    }
    Ok(settings)
}

fn main() -> anyhow::Result<()> {
    let arguments = CommandLine::parse();
    let Some(command) = arguments.command else {
        CommandLine::command().print_long_help()?;
        return Ok(());
    };

    let level = if arguments.quiet {
        LevelFilter::OFF
    } else if arguments.trace {
        LevelFilter::TRACE
    } else if arguments.debug {
        LevelFilter::DEBUG
    } else if arguments.info || matches!(command, Command::Run) {
        LevelFilter::INFO
    } else {
        LevelFilter::WARN
    };
    fmt::Subscriber::builder()
        .with_max_level(level)
        .json()
        .flatten_event(true)
        .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE)
        .with_target(matches!(level, LevelFilter::DEBUG | LevelFilter::TRACE))
        .with_file(level == LevelFilter::TRACE)
        .with_line_number(level == LevelFilter::TRACE)
        .with_writer(std::io::stderr)
        .init();

    let Some(path) = arguments.config else {
        anyhow::bail!("--config is required");
    };
    let settings = read_configuration(&path, matches!(command, Command::Check))?;
    match command {
        Command::Check => {
            let count = settings.wake.len();
            let rules = if count == 1 {
                "wake rule"
            } else {
                "wake rules"
            };
            let skills = match &settings.skills {
                Some(skills) => script::skills_in(&skills.directory).len(),
                None => 0,
            };
            let skill_word = if skills == 1 { "skill" } else { "skills" };
            println!("agent.toml is fine: {count} {rules}, {skills} {skill_word}");
            Ok(())
        }
        Command::Run => run(settings),
    }
}

/// Starts the agent: `agent.db` and the index opened, the task driving core, and the loop on a
/// blocking thread, until SIGTERM or SIGINT, which end it at once, as every phase is already saved.
fn run(settings: Settings) -> anyhow::Result<()> {
    std::fs::create_dir_all(&settings.directory).with_context(|| {
        format!(
            "Could not make the directory {}",
            settings.directory.display()
        )
    })?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Could not start the runtime")?;
    let store = Store::open(&settings.directory).context("Could not open agent.db")?;
    let mut index = Index::open(&settings.directory).context("Could not open the search index")?;
    let mut covered = index.covered();
    if covered.notes == 0 {
        let notes = store
            .all_notes()
            .context("Could not read the memory notes")?;
        for note in notes {
            covered.notes = covered.notes.max(note.updated_at);
            index
                .put_note(note)
                .context("Could not rebuild the note index")?;
        }
        if covered.notes != 0 {
            index
                .commit(covered)
                .context("Could not save the rebuilt note index")?;
        }
    }
    let (asks, ask_inbox) = mpsc::unbounded_channel();
    let shared = Arc::new(Shared {
        store: Mutex::new(store),
        view: Mutex::new(View::default()),
        chats_changed: Notify::new(),
        index: Mutex::new(index),
        wake: Notify::new(),
        asks,
        directory: settings.directory.clone(),
        plain: settings.search.plain,
        next_handle: Mutex::new(0),
    });
    let (driver, inbox) = Driver::new(
        shared.clone(),
        &settings.url,
        &settings.token,
        &settings.words,
    );
    let env = Arc::new(Env::new(
        shared,
        Arc::new(settings),
        runtime.handle().clone(),
    ));
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "keryx-agent starts");
    runtime.spawn(driver.run(inbox, ask_inbox));
    let looping = runtime.spawn_blocking(move || keryx_agent::run::run(env));
    let stopped = runtime.block_on(async {
        let terminate = async {
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(mut signal) => {
                    signal.recv().await;
                }
                Err(_) => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => "SIGINT",
            _ = terminate => "SIGTERM",
            _ = looping => "the loop ended",
        }
    });
    tracing::info!(reason = stopped, "keryx-agent stops");
    runtime.shutdown_timeout(Duration::ZERO);
    Ok(())
}
