//! The `keryx` binary. `keryx serve` is the server, and every other subcommand is a client of it: it
//! tells the server what to do and shows the reply, and never opens the database.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use clap::{CommandFactory, Parser, Subcommand};
use keryx::api::{self, Settings};
use keryx_core::protocol::CreateAccountBody;
use serde_json::{Value, json};
use tanzim::validator::{ByteSize, Duration as DurationValue, StaticMap};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::{self, format::FmtSpan};

/// A self-hosted messenger: chat, calls, MCP and end-to-end encryption.
///
/// `keryx serve` runs the server. Every other command tells a running server what to do and shows its
/// reply. Every command needs --config.
#[derive(Parser)]
#[command(name = "keryx", version)]
struct CommandLine {
    /// The configuration file. Required on every command, with no default.
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
    /// Do not check the server's certificate. Every command accepts this, before or after its name,
    /// and it changes nothing where no certificate would be checked.
    #[arg(long, global = true)]
    insecure: bool,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the server, until SIGINT or SIGTERM. Creates the database the first time.
    Serve,
    /// Manage accounts, on a running server.
    User {
        /// The server, as HOST, HOST:PORT or https://HOST. The default is 127.0.0.1, with the port
        /// from `listen` in the configuration.
        #[arg(long, global = true, value_name = "HOST")]
        remote: Option<String>,
        /// Print the server's whole reply as JSON.
        #[arg(long, global = true)]
        json: bool,
        #[command(subcommand)]
        command: Option<UserCommand>,
    },
}

#[derive(Subcommand)]
enum UserCommand {
    /// Create an account, and print the one-time link where its person sets their password.
    Add {
        /// Lowercase a-z, 0-9 and _, not starting with a digit.
        username: String,
        /// Give the account the admin role.
        #[arg(long)]
        admin: bool,
        /// Any language. Shown to others and searched in the directory.
        #[arg(long, value_name = "NAME")]
        display_name: Option<String>,
    },
}

/// Reads `server.toml`. Flattening tanzim's error with `{:#}` is the point: that form carries the
/// source, line, column and caret, and wrapping it as a source throws them away.
fn read_configuration(path: &Path) -> anyhow::Result<Settings> {
    let body = StaticMap::new()
        .optional("text", ByteSize::new())
        .optional("reaction", ByteSize::new())
        .optional("voice", ByteSize::new())
        .optional("image", ByteSize::new())
        .optional("music", ByteSize::new())
        .optional("video", ByteSize::new())
        .optional("doc", ByteSize::new())
        .optional("file", ByteSize::new())
        .optional("call", ByteSize::new())
        .optional("media_bundle", ByteSize::new())
        .optional("draft", ByteSize::new());
    let upload = StaticMap::new()
        .optional("voice", ByteSize::new())
        .optional("image", ByteSize::new())
        .optional("music", ByteSize::new())
        .optional("video", ByteSize::new())
        .optional("doc", ByteSize::new())
        .optional("file", ByteSize::new());
    let schema = StaticMap::new()
        .required_any("listen")
        .required_any("public_url")
        .optional_any("name")
        .optional_any("max_devices")
        .required(
            "database",
            StaticMap::new().required_any("type").required_any("path"),
        )
        .optional("operator", StaticMap::new().required_any("token"))
        .optional(
            "registration",
            StaticMap::new()
                .optional_any("allow")
                .optional_any("password_min_length")
                .optional_any("username_min_length"),
        )
        .optional(
            "session",
            StaticMap::new().optional("lifetime", DurationValue::new().to_int()),
        )
        .optional(
            "chat",
            StaticMap::new().optional("idle_after", DurationValue::new().to_int()),
        )
        .optional(
            "calls",
            StaticMap::new()
                .optional("test_ring", DurationValue::new().to_int())
                .optional("test_grace", DurationValue::new().to_int()),
        )
        .required("www", StaticMap::new().required_any("directory"))
        .required("files", StaticMap::new().required_any("directory"))
        .optional(
            "limits",
            StaticMap::new()
                .optional("websocket_frame", ByteSize::new())
                .optional("rest_body", ByteSize::new())
                .optional_any("history_page")
                .optional_any("outgoing_frames")
                .optional("ping_after", DurationValue::new().to_int())
                .optional("ping_every", DurationValue::new().to_int())
                .optional("ping_timeout", DurationValue::new().to_int())
                .optional_any("key_packages")
                .optional("call_frame", ByteSize::new())
                .optional("call_cache", ByteSize::new())
                .optional("body", body)
                .optional("upload", upload),
        )
        .optional(
            "tcp",
            StaticMap::new()
                .optional("send_timeout", DurationValue::new().to_int())
                .optional("user_timeout", DurationValue::new().to_int())
                .optional_any("nodelay")
                .optional("keepalive_idle", DurationValue::new().to_int())
                .optional("keepalive_interval", DurationValue::new().to_int())
                .optional_any("keepalive_retries")
                .optional("send_buffer", ByteSize::new())
                .optional("receive_buffer", ByteSize::new())
                .optional_any("backlog"),
        )
        .optional(
            "tls",
            StaticMap::new()
                .required_any("certificate")
                .required_any("key"),
        )
        .optional(
            "captcha",
            StaticMap::new()
                .optional_any("length")
                .optional_any("alphabet")
                .optional("ttl", DurationValue::new().to_int())
                .optional_any("difficulty")
                .optional_any("style")
                .optional_any("case_sensitive")
                .optional("sweep_every", DurationValue::new().to_int())
                .optional("keep_for", DurationValue::new().to_int())
                .optional_any("test_answer")
                .optional_any("pass_code"),
        );
    let settings: Settings = match tanzim::Config::default()
        .with_source(tanzim::source::file(path.to_string_lossy()))
        .with_schema(schema)
        .try_deserialize()
    {
        Ok(settings) => settings,
        Err(error) => anyhow::bail!("Could not read the configuration: {error:#}"),
    };
    if settings.database.kind != "sqlite" {
        anyhow::bail!(
            "database.type is {:?}, and this build only has \"sqlite\"",
            settings.database.kind
        );
    }
    if !settings.public_url.starts_with("http://") && !settings.public_url.starts_with("https://") {
        anyhow::bail!("public_url must start with http:// or https://");
    }
    let time_list = [
        ("limits.ping_after", Some(settings.limits.ping_after)),
        ("limits.ping_every", Some(settings.limits.ping_every)),
        ("limits.ping_timeout", Some(settings.limits.ping_timeout)),
        ("tcp.send_timeout", Some(settings.tcp.send_timeout)),
        ("tcp.user_timeout", settings.tcp.user_timeout),
        ("tcp.keepalive_idle", settings.tcp.keepalive_idle),
        ("tcp.keepalive_interval", settings.tcp.keepalive_interval),
        ("calls.test_ring", settings.calls.test_ring),
        ("calls.test_grace", settings.calls.test_grace),
    ];
    for (key, value) in time_list {
        if value == Some(0) {
            anyhow::bail!("{key} must be above zero");
        }
    }
    let mut path_list = vec![
        ("database.path", &settings.database.path),
        ("www.directory", &settings.www.directory),
        ("files.directory", &settings.files.directory),
    ];
    if let Some(tls) = &settings.tls {
        path_list.push(("tls.certificate", &tls.certificate));
        path_list.push(("tls.key", &tls.key));
    }
    for (key, value) in path_list {
        if !value.is_absolute() {
            anyhow::bail!("{key} must be an absolute path, and is {value:?}");
        }
    }
    Ok(settings)
}

/// Where a subcommand connects: its origin, and whether to check the certificate. On the default
/// 127.0.0.1 it does not, since the traffic never leaves the machine and a certificate is issued for
/// a name. With --remote it always does, unless --insecure.
fn server_origin(settings: &Settings, remote: Option<String>, insecure: bool) -> (String, bool) {
    let port = settings.listen.port();
    let mut scheme = if settings.tls.is_some() {
        "https"
    } else {
        "http"
    };
    let Some(remote) = remote else {
        return (format!("{scheme}://127.0.0.1:{port}"), false);
    };
    let mut host = remote.as_str();
    if let Some(rest) = host.strip_prefix("https://") {
        scheme = "https";
        host = rest;
    } else if let Some(rest) = host.strip_prefix("http://") {
        scheme = "http";
        host = rest;
    }
    let host = host.trim_end_matches('/');
    let after_address = match host.rfind(']') {
        Some(at) => &host[at..],
        None => host,
    };
    let origin = if after_address.contains(':') {
        format!("{scheme}://{host}")
    } else {
        format!("{scheme}://{host}:{port}")
    };
    (origin, !insecure)
}

/// One request to the server, with the operator's token. A 4xx here is an answer and not a failure:
/// its body carries the code and message, and `--json` prints it whole.
fn request_server(
    settings: &Settings,
    remote: Option<String>,
    insecure: bool,
    route: &str,
    body: &Value,
) -> anyhow::Result<Value> {
    let Some(operator) = &settings.operator else {
        anyhow::bail!("The configuration has no [operator] token");
    };
    let (origin, verify) = server_origin(settings, remote, insecure);
    let mut tls = ureq::tls::TlsConfig::builder();
    if !verify {
        tls = tls.disable_verification(true);
    }
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(120)))
        .tls_config(tls.build())
        .build()
        .new_agent();
    let url = format!("{origin}{route}");
    let span = tracing::info_span!("Client", op = "client");
    let _entered = span.enter();
    let started = std::time::Instant::now();
    let mut response = agent
        .post(&url)
        .header("Authorization", &format!("Bearer {}", operator.token))
        .send_json(body)
        .with_context(|| format!("Could not reach {origin}"))?;
    let status = response.status().as_u16();
    let trace = match response.headers().get("x-trace-id") {
        Some(value) => match value.to_str() {
            Ok(text) => text.to_string(),
            Err(_) => String::new(),
        },
        None => String::new(),
    };
    tracing::info!(
        msg = "HTTP client request",
        http_method = "POST",
        http_path = route,
        http_status = status,
        total_ms = started.elapsed().as_millis() as u64,
        trace = %trace
    );
    response
        .body_mut()
        .read_json()
        .with_context(|| format!("{origin} did not answer as a keryx server"))
}

fn run_user_add(
    settings: &Settings,
    remote: Option<String>,
    insecure: bool,
    json_output: bool,
    username: String,
    admin: bool,
    display_name: Option<String>,
) -> anyhow::Result<()> {
    let reply = request_server(
        settings,
        remote,
        insecure,
        "/api/v1/accounts",
        &serde_json::to_value(CreateAccountBody {
            username,
            admin,
            display_name,
        })?,
    )?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&reply)?);
    }
    if reply["ok"] != json!(true) {
        anyhow::bail!(
            "The server refused: {}",
            reply["message"].as_str().unwrap_or("no reason given")
        );
    }
    if !json_output {
        let Some(link) = reply["data"]["link"].as_str() else {
            anyhow::bail!("The server's reply has no link");
        };
        eprintln!("One-time link, for 7 days:");
        println!("{link}");
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let settings = CommandLine::parse();
    let Some(command) = settings.command else {
        CommandLine::command().print_long_help()?;
        return Ok(());
    };
    if let Command::User { command: None, .. } = command {
        let mut root = CommandLine::command();
        root.build();
        match root.find_subcommand_mut("user") {
            Some(user) => user.print_long_help()?,
            None => root.print_long_help()?,
        }
        return Ok(());
    }

    let level = if settings.quiet {
        LevelFilter::OFF
    } else if settings.trace {
        LevelFilter::TRACE
    } else if settings.debug {
        LevelFilter::DEBUG
    } else if settings.info || matches!(command, Command::Serve) {
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

    let Some(path) = settings.config else {
        anyhow::bail!("--config is required");
    };
    let configuration = read_configuration(&path)?;
    match command {
        Command::Serve => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("Could not start the runtime")?;
            runtime
                .block_on(api::serve(configuration))
                .context("The server stopped")?;
            Ok(())
        }
        Command::User {
            remote,
            json,
            command,
        } => match command {
            Some(UserCommand::Add {
                username,
                admin,
                display_name,
            }) => run_user_add(
                &configuration,
                remote,
                settings.insecure,
                json,
                username,
                admin,
                display_name,
            ),
            None => Ok(()),
        },
    }
}
