//! `graff --mcp`: the questions of `src/query.rs` served to an agent over
//! the Model Context Protocol, each a tool whose answer is the command
//! line's, cut to a budget a call may not exceed.
//!
//! Newline-delimited JSON-RPC over stdin and stdout, answered one request at
//! a time. Dual-era, as ekko's server is: a request whose `_meta` names a
//! protocol version is served statelessly under that revision, 2026-07-28,
//! `server/discover` included, and an `initialize` handshake is served under
//! the legacy revision it negotiates, 2025-11-25 and those before it. Only
//! tools are offered: no prompts, resources or notifications. Written by
//! hand rather than through an SDK, as ekko's is, since the protocol this
//! needs is five methods.
//!
//! Claude Code defers an MCP server's tools behind ToolSearch, loading only
//! their names and the server's instructions at session start, and cuts each
//! of those texts at 2,048 characters: the instructions say when to reach for
//! graff and when grep does better, and the tests hold them and each tool's
//! definition to a budget. stdout carries protocol messages and nothing
//! else. The server exits when stdin closes.

use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::query::{self, Options};
use crate::store::{self, Store};

/// The revisions served per request, by `_meta`.
const MODERN: &[&str] = &["2026-07-28"];

/// Newest first: an `initialize` asking for a version not listed here is
/// answered with the first, which the client may accept or disconnect over.
const LEGACY: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The most tokens a call may ask for, at 4 bytes each: 32,000 bytes, under
/// the 10,000 tokens past which Claude Code warns of an MCP tool's output
/// for text that takes 3.2 bytes a token or more, and under the 50,000
/// characters past which it saves a result to a file instead. 3.2 bytes a
/// token is an estimate for code, not a count.
pub const MOST: usize = 8_000;

/// What `def`, `callers`, `callees`, `outline`, `impact` and `search` take
/// unless told, as on the command line.
const BUDGET: usize = 2_000;

/// What `map` takes unless told: Aider's default, as on the command line.
const MAP_BUDGET: usize = 1_024;

/// What `search` gives unless told, as on the command line.
const LIMIT: usize = 20;

/// The tools, in the order a reader takes them up.
pub const TOOLS: &[&str] = &[
    "def", "callers", "callees", "outline", "impact", "search", "map", "why", "touched",
];

const INSTRUCTIONS: &str = "\
graff answers where code is in a git worktree's Rust, Nix, Bash, Python, C and Markdown, from an index it brings up to date on each call. An answer gives file:line ranges, fits a token budget, and its last line says what a cut left out.

Reach for it before grep or reading whole files when the question is about a name:
- def: where a symbol is defined, with its doc and signature.
- callers, callees: what reaches it and what it reaches, tied by the language's rules for names rather than by text, so another definition of the same name is not counted; uses it cannot tie to one definition come apart, as possible.
- impact: callers of callers, before changing something.
- outline: what a file defines, nested, with lines: Read only the range needed.
- search: definitions by words, when the name is unknown; it says it found nothing when its best match holds too little of them.
- map: what matters most in the worktree, or around files and symbols, for a first look.
- why, touched: the commits, ekko tasks and decisions behind a definition; what a task's commits changed.

grep does better for text that is not a definition's name (strings, messages, comments, config values), for every place a word is written, in languages graff does not read, and in files git ignores.";

/// The client's dialect: the handshake's, or a request of 2026-07-28's.
#[derive(Clone, Copy, PartialEq)]
enum Era {
    Legacy,
    Modern,
}

/// A JSON-RPC error.
struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

impl RpcError {
    fn new(code: i64, message: impl Into<String>) -> RpcError {
        RpcError {
            code,
            message: message.into(),
            data: None,
        }
    }
}

/// Where calls find their worktree: a folder in it, `CLAUDE_PROJECT_DIR`
/// as Claude Code sets it for the servers it starts, else the folder the
/// server was started in.
pub struct Server {
    folder: PathBuf,
}

impl Server {
    pub fn new(folder: PathBuf) -> Server {
        Server { folder }
    }

    /// The reply to one line of input, if it wants one: a notification or a
    /// response does not.
    pub fn handle_line(&self, line: &str) -> Option<Value> {
        let message: Value = match serde_json::from_str(line) {
            Ok(message) => message,
            Err(error) => {
                return Some(error_reply(
                    Value::Null,
                    RpcError::new(-32700, format!("Parse error: {error}")),
                ));
            }
        };
        let Some(message) = message.as_object() else {
            // Batches went with 2025-06-18.
            return Some(error_reply(
                Value::Null,
                RpcError::new(-32600, "Invalid request: not an object"),
            ));
        };
        let id = message.get("id").cloned()?;
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            if message.contains_key("result") || message.contains_key("error") {
                // A response: this server sends no requests to answer.
                return None;
            }
            return Some(error_reply(
                id,
                RpcError::new(-32600, "Invalid request: no method"),
            ));
        };
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        let reply = era(&params).and_then(|era| self.handle(method, &params, era));
        Some(match reply {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err(error) => error_reply(id, error),
        })
    }

    fn handle(&self, method: &str, params: &Value, era: Era) -> Result<Value, RpcError> {
        let result = match method {
            "initialize" => return Ok(initialize(params)),
            "server/discover" if era == Era::Modern => {
                let mut reply = json!({"supportedVersions": MODERN});
                reply.as_object_mut().expect("an object").extend(offer());
                reply
            }
            // 2026-07-28 dropped ping.
            "ping" if era == Era::Legacy => json!({}),
            "tools/list" => json!({"tools": tool_definitions()}),
            "tools/call" => self.call(params)?,
            _ => {
                return Err(RpcError::new(-32601, format!("Method not found: {method}")));
            }
        };
        Ok(match era {
            Era::Modern => complete(method, result),
            Era::Legacy => result,
        })
    }

    /// A tool's answer: the question's, or why it was not asked, which the
    /// model reads to try again. A tool that does not exist is the
    /// protocol's error.
    fn call(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(-32602, "Invalid params: tools/call needs a tool name"))?;
        if !TOOLS.contains(&name) {
            return Err(RpcError::new(-32602, format!("Unknown tool: {name}")));
        }
        let mut args = match params.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(map)) => map.clone(),
            Some(_) => {
                return Err(RpcError::new(
                    -32602,
                    "Invalid params: arguments must be an object",
                ));
            }
        };
        let (text, failed) = match self.tool(name, &mut args) {
            Ok(text) => (text, false),
            Err(why) => (format!("graff: {why}"), true),
        };
        Ok(json!({"content": [{"type": "text", "text": text}], "isError": failed}))
    }

    /// Asks tool `name` its question with `args`, as the command line would.
    fn tool(&self, name: &str, args: &mut Map<String, Value>) -> Result<String, String> {
        let default = if name == "map" { MAP_BUDGET } else { BUDGET };
        let budget = number(args, "budget", 1, MOST, default)?;
        let folder = text(args, "folder")?;
        let asked: Asked = match name {
            "def" | "callers" | "callees" => {
                let symbol = required(args, "symbol")?;
                let query = match name {
                    "def" => query::def,
                    "callers" => query::callers,
                    _ => query::callees,
                };
                Box::new(move |store, root, options| query(store, root, &symbol, options))
            }
            "outline" => {
                let file = required(args, "file")?;
                Box::new(move |store, root, options| query::outline(store, root, &file, options))
            }
            "impact" => {
                let symbol = required(args, "symbol")?;
                let depth = number(args, "depth", 1, query::DEEPEST, 3)?;
                Box::new(move |store, root, options| {
                    query::impact(store, root, &symbol, depth, options)
                })
            }
            "search" => {
                let words = required(args, "words")?;
                let limit = number(args, "limit", 1, u32::MAX as usize, LIMIT)?;
                Box::new(move |store, root, options| {
                    query::search(store, root, &words, limit, options)
                })
            }
            "why" => {
                let symbol = required(args, "symbol")?;
                Box::new(move |store, root, options| {
                    query::why(store, root, &symbol, root, options)
                })
            }
            "touched" => {
                let task = task(args)?;
                Box::new(move |store, root, options| {
                    query::touched(store, root, &task, root, options)
                })
            }
            _ => {
                let focus = texts(args, "focus")?;
                Box::new(move |store, root, options| query::map(store, root, &focus, options))
            }
        };
        if let Some(extra) = args.keys().next() {
            return Err(format!(
                "{name} takes no argument {extra:?}: see its input schema"
            ));
        }
        let folder = match folder {
            Some(folder) => self.folder.join(folder),
            None => self.folder.clone(),
        };
        let root = store::toplevel(&folder).map_err(|error| error.to_string())?;
        let location = store::location()
            .ok_or("no cache folder: neither XDG_CACHE_HOME nor HOME is an absolute path")?;
        let mut store = Store::open(&location).map_err(|error| error.to_string())?;
        let options = Options {
            knob: "budget",
            most: Some(MOST),
            ..Options::new(budget, false)
        };
        asked(&mut store, &root, &options).map_err(|error| error.to_string())
    }
}

/// A question, its arguments taken.
type Asked = Box<dyn FnOnce(&mut Store, &Path, &Options) -> Result<String, query::Error>>;

/// Serves the protocol on stdin and stdout until stdin closes.
pub fn run(folder: PathBuf) -> io::Result<()> {
    let server = Server::new(folder);
    let stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    for line in stdin.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = server.handle_line(&line) {
            writeln!(stdout, "{reply}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Whether a request is of 2026-07-28, served statelessly: its `_meta`
/// names the version, which has to be one served per request, with the
/// client's capabilities beside it. A request without is the handshake's.
fn era(params: &Value) -> Result<Era, RpcError> {
    let meta = params.get("_meta");
    let Some(requested) = meta
        .and_then(|meta| meta.get("io.modelcontextprotocol/protocolVersion"))
        .and_then(Value::as_str)
    else {
        return Ok(Era::Legacy);
    };
    if !MODERN.contains(&requested) {
        // The versions to retry with, per request: the handshake's are not.
        return Err(RpcError {
            code: -32022,
            message: "Unsupported protocol version".to_string(),
            data: Some(json!({"supported": MODERN, "requested": requested})),
        });
    }
    if meta
        .and_then(|meta| meta.get("io.modelcontextprotocol/clientCapabilities"))
        .is_none()
    {
        return Err(RpcError::new(
            -32602,
            "Invalid params: _meta lacks io.modelcontextprotocol/clientCapabilities",
        ));
    }
    Ok(Era::Modern)
}

fn initialize(params: &Value) -> Value {
    let requested = params
        .get("protocolVersion")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let version = LEGACY
        .iter()
        .find(|version| **version == requested)
        .copied()
        .unwrap_or(LEGACY[0]);
    let mut reply = json!({"protocolVersion": version, "serverInfo": server_info()});
    reply.as_object_mut().expect("an object").extend(offer());
    reply
}

/// What the server offers under either revision: tools, a list that never
/// changes while it runs, and the instructions.
fn offer() -> Map<String, Value> {
    let mut offer = Map::new();
    offer.insert("capabilities".into(), json!({"tools": {}}));
    offer.insert("instructions".into(), json!(INSTRUCTIONS));
    offer
}

fn server_info() -> Value {
    json!({"name": "graff", "version": env!("CARGO_PKG_VERSION")})
}

/// The methods whose results are a CacheableResult in the 2026-07-28
/// schema: they carry ttlMs and cacheScope, and Claude Code drops a whole
/// list that lacks them, as ekko found (its task 705).
const CACHEABLE: &[&str] = &["server/discover", "tools/list"];

/// A result as 2026-07-28 wants it: its type, the server's identity, and
/// the caching fields where the method's type requires them.
fn complete(method: &str, mut result: Value) -> Value {
    if let Value::Object(map) = &mut result {
        map.insert("resultType".into(), json!("complete"));
        map.insert(
            "_meta".into(),
            json!({"io.modelcontextprotocol/serverInfo": server_info()}),
        );
        if CACHEABLE.contains(&method) {
            // Stale at once and never shared, the official SDKs' default: a
            // list kept past this process could hide an upgrade of graff.
            map.insert("ttlMs".into(), json!(0));
            map.insert("cacheScope".into(), json!("private"));
        }
    }
    result
}

fn error_reply(id: Value, error: RpcError) -> Value {
    let mut body = json!({"code": error.code, "message": error.message});
    if let Some(data) = error.data {
        body["data"] = data;
    }
    json!({"jsonrpc": "2.0", "id": id, "error": body})
}

/// Takes a text argument, if given.
fn text(args: &mut Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match args.remove(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(other) => Err(format!("{key} must be a string, not {other}")),
    }
}

/// Takes a text argument that must be given, and not be blank.
fn required(args: &mut Map<String, Value>, key: &str) -> Result<String, String> {
    match text(args, key)? {
        Some(text) if !text.trim().is_empty() => Ok(text),
        _ => Err(format!("{key} is required")),
    }
}

/// Takes a whole number from `low` to `high`, `default` if not given.
fn number(
    args: &mut Map<String, Value>,
    key: &str,
    low: usize,
    high: usize,
    default: usize,
) -> Result<usize, String> {
    let value = match args.remove(key) {
        None | Some(Value::Null) => return Ok(default),
        Some(value) => value,
    };
    match value.as_u64().and_then(|n| usize::try_from(n).ok()) {
        Some(n) if (low..=high).contains(&n) => Ok(n),
        _ => Err(format!(
            "{key} must be a whole number from {low} to {high}, not {value}"
        )),
    }
}

/// Takes a task's id, as a text or a number: `"28"` or `28`.
fn task(args: &mut Map<String, Value>) -> Result<String, String> {
    match args.remove("task") {
        Some(Value::Number(id)) => Ok(id.to_string()),
        Some(other) => {
            args.insert("task".into(), other);
            required(args, "task")
        }
        None => Err("task is required".to_string()),
    }
}

/// Takes a list of texts, empty if not given.
fn texts(args: &mut Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    match args.remove(key) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .into_iter()
            .map(|item| match item {
                Value::String(text) => Ok(text),
                other => Err(format!("{key} must hold strings, not {other}")),
            })
            .collect(),
        Some(other) => Err(format!("{key} must be a list of strings, not {other}")),
    }
}

/// The tools' definitions, as tools/list gives them.
pub fn tool_definitions() -> Value {
    let symbol = json!({"type": "string", "description": "load, Storage::load, src/store.rs:Storage::load, src/store.rs:120 for the definition around a line, a file by its path, or a Markdown section by its heading or anchor: readme.md#usage"});
    let folder = json!({"type": "string", "description": "A folder in the worktree to ask about, from the session's. Absent: the session's."});
    let budget = |default: usize| json!({"type": "integer", "minimum": 1, "maximum": MOST, "description": format!("The tokens the answer may take, at 4 bytes each. Absent: {default}.")});
    let object = |properties: Value, required: &[&str]| json!({"type": "object", "properties": properties, "required": required, "additionalProperties": false});
    let about_symbol = || {
        object(
            json!({"symbol": symbol, "folder": folder, "budget": budget(BUDGET)}),
            &["symbol"],
        )
    };
    let mut tools = json!([
        {
            "name": "def",
            "description": "Where a symbol is defined: its lines, the first sentence of its doc, its signature, and a type's or trait's impl blocks.",
            "inputSchema": about_symbol(),
        },
        {
            "name": "callers",
            "description": "What reaches a symbol: the calls, references and imports tied to it, by the definition each is in; then those that may reach it, by name alone.",
            "inputSchema": about_symbol(),
        },
        {
            "name": "callees",
            "description": "What a symbol reaches: the definitions its calls and references are tied to, those they may reach, and the names it uses from outside the worktree.",
            "inputSchema": about_symbol(),
        },
        {
            "name": "outline",
            "description": "The symbols a file defines, nested, with their lines.",
            "inputSchema": object(
                json!({
                    "file": {"type": "string", "description": "A file by its path from the worktree's top, or the end of one: store.rs"},
                    "folder": folder,
                    "budget": budget(BUDGET),
                }),
                &["file"],
            ),
        },
        {
            "name": "impact",
            "description": "What reaches a symbol from afar: its callers, theirs, and so on, through the uses tied to one definition.",
            "inputSchema": object(
                json!({
                    "symbol": symbol,
                    "depth": {"type": "integer", "minimum": 1, "maximum": query::DEEPEST, "description": "How many levels of callers to follow. Absent: 3."},
                    "folder": folder,
                    "budget": budget(BUDGET),
                }),
                &["symbol"],
            ),
        },
        {
            "name": "search",
            "description": "The definitions some words find, best first: ranked by BM25 over their names, split at underscores and case, their paths and their docs. None when the best holds too little of the words, and then it says which words no definition holds, to ask in others.",
            "inputSchema": object(
                json!({
                    "words": {"type": "string", "description": "Words, names or both: storage file, parseMap, open_store"},
                    "limit": {"type": "integer", "minimum": 1, "description": format!("How many definitions to give at most. Absent: {LIMIT}.")},
                    "folder": folder,
                    "budget": budget(BUDGET),
                }),
                &["words"],
            ),
        },
        {
            "name": "map",
            "description": "The definitions that matter most, by signature under their files, ranked by PageRank over the graph of uses, around the files and symbols in focus: what to read first.",
            "inputSchema": object(
                json!({
                    "focus": {"type": "array", "items": {"type": "string"}, "description": "Files or symbols to center the map on, which it leaves out: src/store.rs, Storage::load. Absent: the whole worktree."},
                    "folder": folder,
                    "budget": budget(MAP_BUDGET),
                }),
                &[],
            ),
        },
        {
            "name": "why",
            "description": "Why a definition is as it is: the commits that changed it, newest first, the ekko tasks their Ekko: trailers name, and those tasks' decisions and gotchas; or why the board could not say.",
            "inputSchema": about_symbol(),
        },
        {
            "name": "touched",
            "description": "What a task changed: the commits whose Ekko: trailer names it, then the definitions they changed, each once, where it is now.",
            "inputSchema": object(
                json!({
                    "task": {"type": "string", "description": "Its id, as in Ekko: 28."},
                    "folder": folder,
                    "budget": budget(BUDGET),
                }),
                &["task"],
            ),
        },
    ]);
    // Each only reads: the index it writes is graff's cache.
    for tool in tools.as_array_mut().expect("an array") {
        tool["annotations"] =
            json!({"readOnlyHint": true, "idempotentHint": true, "openWorldHint": false});
    }
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Claude Code cuts a server's instructions and each tool's description
    /// at 2,048 characters.
    const CUT: usize = 2_048;

    /// What a session pays for graff, in characters: the instructions at
    /// its start, and the definitions on the ToolSearch that loads them.
    const INSTRUCTIONS_BUDGET: usize = 1_300;
    const DEFINITIONS_BUDGET: usize = 7_500;

    #[test]
    fn what_a_session_pays_for_stays_under_its_budget() {
        let instructions = INSTRUCTIONS.chars().count();
        assert!(
            instructions <= INSTRUCTIONS_BUDGET.min(CUT),
            "the instructions take {instructions} characters"
        );
        let definitions = tool_definitions();
        let size = definitions.to_string().chars().count();
        assert!(
            size <= DEFINITIONS_BUDGET,
            "the definitions take {size} characters"
        );
        for tool in definitions.as_array().unwrap() {
            let description = tool["description"].as_str().unwrap();
            assert!(description.chars().count() <= CUT, "{description}");
        }
    }

    #[test]
    fn every_tool_is_defined_once_in_order_and_named_in_the_instructions() {
        let definitions = tool_definitions();
        let names: Vec<&str> = definitions
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, TOOLS);
        for name in TOOLS {
            assert!(
                INSTRUCTIONS.contains(&format!("{name}:"))
                    || INSTRUCTIONS.contains(&format!("{name},")),
                "the instructions say nothing of {name}"
            );
        }
    }

    #[test]
    fn every_argument_a_schema_names_is_taken_and_no_other() {
        let server = Server::new(PathBuf::from("/"));
        for tool in tool_definitions().as_array().unwrap() {
            let name = tool["name"].as_str().unwrap();
            let schema = &tool["inputSchema"];
            let given = || {
                let mut args = Map::new();
                for key in schema["required"].as_array().unwrap() {
                    args.insert(key.as_str().unwrap().into(), json!("x"));
                }
                args
            };
            for (key, property) in schema["properties"].as_object().unwrap() {
                // A value of the wrong type is refused by the argument's name.
                let mut args = given();
                args.insert(key.clone(), json!({"wrong": true}));
                let refused = server.tool(name, &mut args).unwrap_err();
                assert!(refused.starts_with(key.as_str()), "{name} {key}: {refused}");
                assert!(property["description"].is_string(), "{name} {key}");
            }
            let mut args = given();
            args.insert("nonsense".into(), json!("x"));
            let refused = server.tool(name, &mut args).unwrap_err();
            assert_eq!(
                refused,
                format!("{name} takes no argument \"nonsense\": see its input schema")
            );
        }
    }

    #[test]
    fn a_budget_past_the_most_a_call_takes_is_refused() {
        let server = Server::new(PathBuf::from("/"));
        for (budget, why) in [
            (
                json!(0),
                "budget must be a whole number from 1 to 8000, not 0",
            ),
            (
                json!(8001),
                "budget must be a whole number from 1 to 8000, not 8001",
            ),
            (
                json!(2.5),
                "budget must be a whole number from 1 to 8000, not 2.5",
            ),
            (
                json!(-1),
                "budget must be a whole number from 1 to 8000, not -1",
            ),
        ] {
            let mut args = Map::new();
            args.insert("symbol".into(), json!("x"));
            args.insert("budget".into(), budget);
            assert_eq!(server.tool("def", &mut args).unwrap_err(), why);
        }
    }

    fn reply(line: Value) -> Value {
        Server::new(PathBuf::from("/"))
            .handle_line(&line.to_string())
            .expect("a reply")
    }

    fn modern(method: &str, params: Value) -> Value {
        let mut params = params;
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "test", "version": "1"},
        });
        reply(json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params}))
    }

    #[test]
    fn the_handshake_negotiates_a_legacy_revision() {
        for (asked, given) in [
            ("2025-11-25", "2025-11-25"),
            ("2025-06-18", "2025-06-18"),
            ("2026-07-28", "2025-11-25"),
            ("1900-01-01", "2025-11-25"),
        ] {
            let answer = reply(
                json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": asked, "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}),
            );
            let result = &answer["result"];
            assert_eq!(result["protocolVersion"], given);
            assert_eq!(result["serverInfo"]["name"], "graff");
            assert_eq!(result["capabilities"], json!({"tools": {}}));
            assert_eq!(result["instructions"], INSTRUCTIONS);
            assert!(result.get("resultType").is_none());
        }
        let ping = reply(json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}));
        assert_eq!(ping["result"], json!({}));
        let listed = reply(json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}));
        assert_eq!(listed["result"], json!({"tools": tool_definitions()}));
    }

    #[test]
    fn a_request_of_2026_07_28_is_served_on_its_own() {
        let discovered = modern("server/discover", json!({}));
        let result = &discovered["result"];
        assert_eq!(result["supportedVersions"], json!(["2026-07-28"]));
        assert_eq!(result["capabilities"], json!({"tools": {}}));
        assert_eq!(result["instructions"], INSTRUCTIONS);
        assert_eq!(result["resultType"], "complete");
        assert_eq!(
            result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "graff"
        );
        assert_eq!(result["ttlMs"], 0);
        assert_eq!(result["cacheScope"], "private");
        let listed = modern("tools/list", json!({}));
        assert_eq!(listed["result"]["tools"], tool_definitions());
        assert_eq!(listed["result"]["ttlMs"], 0);
        let called = modern("tools/call", json!({"name": "def", "arguments": {}}));
        let result = &called["result"];
        assert_eq!(result["resultType"], "complete");
        assert_eq!(result["isError"], true);
        assert_eq!(result["content"][0]["text"], "graff: symbol is required");
        assert!(result.get("ttlMs").is_none());
        // ping went with the handshake.
        assert_eq!(modern("ping", json!({}))["error"]["code"], -32601);
    }

    #[test]
    fn a_revision_not_served_per_request_is_refused_with_those_that_are() {
        let answer = reply(
            json!({"jsonrpc": "2.0", "id": 4, "method": "tools/list", "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2025-11-25", "io.modelcontextprotocol/clientCapabilities": {}}}}),
        );
        assert_eq!(
            answer["error"],
            json!({"code": -32022, "message": "Unsupported protocol version", "data": {"supported": ["2026-07-28"], "requested": "2025-11-25"}})
        );
        let answer = reply(
            json!({"jsonrpc": "2.0", "id": 5, "method": "tools/list", "params": {"_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28"}}}),
        );
        assert_eq!(answer["error"]["code"], -32602);
    }

    #[test]
    fn what_is_not_a_request_gets_no_reply_or_the_protocols_error() {
        let server = Server::new(PathBuf::from("/"));
        assert!(
            server
                .handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .is_none()
        );
        assert!(
            server
                .handle_line(r#"{"jsonrpc":"2.0","id":9,"result":{}}"#)
                .is_none()
        );
        let error = |line: &str| server.handle_line(line).unwrap()["error"]["code"].clone();
        assert_eq!(error("{"), -32700);
        assert_eq!(error("[]"), -32600);
        assert_eq!(error(r#"{"jsonrpc":"2.0","id":1}"#), -32600);
        assert_eq!(
            error(r#"{"jsonrpc":"2.0","id":1,"method":"prompts/list"}"#),
            -32601
        );
        assert_eq!(
            error(r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"grep"}}"#),
            -32602
        );
        assert_eq!(
            error(
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"def","arguments":[]}}"#
            ),
            -32602
        );
    }
}
